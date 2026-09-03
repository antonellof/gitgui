//! Commit graph lane assignment, gitk style (docs/SPEC.md 3.3).

use git2::Oid;

use super::repo::CommitRow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// Another lane merging into this commit (from_lane -> commit lane).
    Merge,
    /// This commit forking out to a parent in another lane (commit lane -> to_lane).
    Fork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from_lane: usize,
    pub to_lane: usize,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RowLayout {
    pub lane: usize,
    pub color: usize,
    /// Edges leaving this row downwards (to the next row).
    pub edges: Vec<Edge>,
    /// Lanes that pass straight through this row (excluding the commit lane).
    pub through: Vec<(usize, usize)>,
    /// Number of lanes active at this row, for column width.
    pub width: usize,
    pub is_merge: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphLayout {
    pub rows: Vec<RowLayout>,
    pub max_lanes: usize,
}

pub const PALETTE_SIZE: usize = 8;

struct Lane {
    expects: Oid,
    color: usize,
}

pub fn layout(commits: &[CommitRow]) -> GraphLayout {
    let mut active: Vec<Option<Lane>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    let mut max_lanes = 0;
    let mut next_color = 0usize;

    for c in commits {
        // Lanes expecting this commit.
        let matches: Vec<usize> = active
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_ref().is_some_and(|l| l.expects == c.oid))
            .map(|(i, _)| i)
            .collect();
        let mut edges = Vec::new();
        let (lane, color) = match matches.first() {
            Some(&l) => (l, active[l].as_ref().map(|x| x.color).unwrap_or(0)),
            None => {
                let l = first_free(&mut active);
                let color = next_color;
                next_color = (next_color + 1) % PALETTE_SIZE;
                active[l] = Some(Lane { expects: c.oid, color });
                (l, color)
            }
        };
        // Other lanes that expected this commit merge into it; they end here.
        for &other in matches.iter().skip(1) {
            edges.push(Edge { from_lane: other, to_lane: lane, kind: EdgeKind::Merge });
            active[other] = None;
        }
        // Lanes that pass straight through this row.
        let through: Vec<(usize, usize)> = active
            .iter()
            .enumerate()
            .filter(|(i, l)| *i != lane && l.is_some())
            .map(|(i, l)| (i, l.as_ref().map(|x| x.color).unwrap_or(0)))
            .collect();
        let width_before = active.len();

        if c.parents.is_empty() {
            active[lane] = None;
        } else {
            active[lane] = Some(Lane { expects: c.parents[0], color });
            for p in &c.parents[1..] {
                if let Some(l) = active.iter().position(|x| x.as_ref().is_some_and(|x| x.expects == *p)) {
                    edges.push(Edge { from_lane: lane, to_lane: l, kind: EdgeKind::Fork });
                } else {
                    let l = first_free(&mut active);
                    let pc = next_color;
                    next_color = (next_color + 1) % PALETTE_SIZE;
                    active[l] = Some(Lane { expects: *p, color: pc });
                    edges.push(Edge { from_lane: lane, to_lane: l, kind: EdgeKind::Fork });
                }
            }
        }
        while active.last().is_some_and(|l| l.is_none()) {
            active.pop();
        }
        let width = width_before.max(active.len()).max(lane + 1);
        max_lanes = max_lanes.max(width);
        rows.push(RowLayout { lane, color, edges, through, width, is_merge: c.parents.len() > 1 });
    }
    GraphLayout { rows, max_lanes }
}

fn first_free(active: &mut Vec<Option<Lane>>) -> usize {
    match active.iter().position(|l| l.is_none()) {
        Some(i) => i,
        None => {
            active.push(None);
            active.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(n: u8) -> Oid {
        let mut bytes = [0u8; 20];
        bytes[0] = n;
        Oid::from_bytes(&bytes).unwrap()
    }

    fn row(id: u8, parents: &[u8]) -> CommitRow {
        CommitRow {
            oid: oid(id),
            short: format!("{id:07x}"),
            parents: parents.iter().map(|p| oid(*p)).collect(),
            summary: String::new(),
            author: String::new(),
            email: String::new(),
            time: 0,
            refs: Vec::new(),
        }
    }

    #[test]
    fn linear_history_single_lane() {
        let commits = vec![row(3, &[2]), row(2, &[1]), row(1, &[])];
        let g = layout(&commits);
        assert_eq!(g.max_lanes, 1);
        assert!(g.rows.iter().all(|r| r.lane == 0 && r.edges.is_empty()));
        assert!(g.rows.iter().all(|r| r.color == 0));
    }

    #[test]
    fn one_merge() {
        // 4 = merge(3, 2); 3 -> 1; 2 -> 1; 1 root
        let commits = vec![row(4, &[3, 2]), row(3, &[1]), row(2, &[1]), row(1, &[])];
        let g = layout(&commits);
        assert_eq!(g.max_lanes, 2);
        assert_eq!(g.rows[0].lane, 0);
        assert!(g.rows[0].is_merge);
        assert_eq!(g.rows[0].edges, vec![Edge { from_lane: 0, to_lane: 1, kind: EdgeKind::Fork }]);
        assert_eq!(g.rows[1].lane, 0);
        assert_eq!(g.rows[1].through, vec![(1, 1)]);
        // commit 2 sits in lane 1; its first parent keeps lane 1 open until
        // the root row, where lane 1 merges into lane 0.
        assert_eq!(g.rows[2].lane, 1);
        assert!(g.rows[2].edges.is_empty());
        assert_eq!(g.rows[2].through, vec![(0, 0)]);
        // root: lane 0, lane 1 merged into it
        assert_eq!(g.rows[3].lane, 0);
        assert_eq!(g.rows[3].edges, vec![Edge { from_lane: 1, to_lane: 0, kind: EdgeKind::Merge }]);
        assert_eq!(g.rows[3].width, 2);
    }

    #[test]
    fn octopus_merge() {
        // 5 = merge(4, 3, 2), all -> 1
        let commits = vec![row(5, &[4, 3, 2]), row(4, &[1]), row(3, &[1]), row(2, &[1]), row(1, &[])];
        let g = layout(&commits);
        assert_eq!(g.max_lanes, 3);
        assert_eq!(g.rows[0].edges.len(), 2);
        assert!(g.rows[0].edges.iter().all(|e| e.kind == EdgeKind::Fork && e.from_lane == 0));
        assert_eq!(g.rows[4].edges.iter().filter(|e| e.kind == EdgeKind::Merge).count(), 2);
        // Lanes get distinct colors.
        let colors: std::collections::HashSet<usize> = g.rows[1..4].iter().map(|r| r.color).collect();
        assert_eq!(colors.len(), 3);
    }

    #[test]
    fn two_independent_roots() {
        let commits = vec![row(4, &[3]), row(3, &[]), row(2, &[1]), row(1, &[])];
        let g = layout(&commits);
        // The first root frees its lane, so the second history reuses lane 0.
        assert_eq!(g.rows[0].lane, 0);
        assert_eq!(g.rows[1].lane, 0);
        assert_eq!(g.rows[2].lane, 0);
        assert_eq!(g.rows[3].lane, 0);
        assert_ne!(g.rows[0].color, g.rows[2].color);
        assert_eq!(g.max_lanes, 1);
    }

    #[test]
    fn branch_started_later_keeps_lane_open() {
        // 5 -> 4 -> 2 -> 1 in lane 0; 3 -> 1 appears in the middle on lane 1.
        let commits = vec![row(5, &[4]), row(4, &[2]), row(3, &[1]), row(2, &[1]), row(1, &[])];
        let g = layout(&commits);
        assert_eq!(g.rows[2].lane, 1);
        assert_eq!(g.rows[3].lane, 0);
        assert_eq!(g.rows[3].through, vec![(1, g.rows[2].color)]);
        assert_eq!(g.rows[4].edges, vec![Edge { from_lane: 1, to_lane: 0, kind: EdgeKind::Merge }]);
    }
}
