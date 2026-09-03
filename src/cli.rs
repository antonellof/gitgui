//! Command line parsing.

use std::path::PathBuf;

pub const USAGE: &str = "usage: gitgui [options] [path]
       gitgui ls
       gitgui action [--pid <n>] <json>

  --split <left|right|up|down>  open in a new terminal split
  --size <0.2..0.95>            fraction of the pane the split takes (reserved)
  --probe                    print detected terminal capabilities and exit
  --dump-input               print decoded input events, Ctrl+C to exit
  --headless-frame <out.png> render one frame to a PNG and exit
  --size <WxH>               frame size for --headless-frame (default 1600x1000)
  --scale <1|1.5|2>          override pixels per point
  --font-size <N>            UI font size in points (default 13)
  --repo <path>              same as the positional path
  --no-shm                   force the direct (base64 + zlib) transport
  -h, --help                 show this help";

pub enum Mode {
    Run,
    List,
    Action { json: String, pid: Option<u32> },
}

pub struct Cli {
    pub mode: Mode,
    pub probe: bool,
    pub dump_input: bool,
    pub no_shm: bool,
    pub crash: bool,
    pub headless: Option<PathBuf>,
    pub size: (u32, u32),
    pub split: Option<String>,
    pub split_size: Option<f32>,
    pub scale: Option<f32>,
    pub font_size: Option<f32>,
    pub path: Option<PathBuf>,
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Cli, String> {
    let args: Vec<String> = args.into_iter().collect();
    if args.first().map(String::as_str) == Some("ls") {
        return Ok(Cli {
            mode: Mode::List,
            probe: false,
            dump_input: false,
            no_shm: false,
            crash: false,
            headless: None,
            size: (1600, 1000),
            split: None,
            split_size: None,
            scale: None,
            font_size: None,
            path: None,
        });
    }
    if args.first().map(String::as_str) == Some("action") {
        let mut pid = None;
        let mut json = None;
        let mut it = args.iter().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--pid" => {
                    let v = it
                        .next()
                        .ok_or_else(|| format!("--pid needs a value\n{USAGE}"))?;
                    pid = Some(v.parse().map_err(|_| format!("bad pid {v:?}"))?);
                }
                other if other.starts_with('-') => {
                    return Err(format!("unknown argument {other:?}\n{USAGE}"))
                }
                other => json = Some(other.to_string()),
            }
        }
        let json = json.ok_or_else(|| format!("action needs a json argument\n{USAGE}"))?;
        return Ok(Cli {
            mode: Mode::Action { json, pid },
            probe: false,
            dump_input: false,
            no_shm: false,
            crash: false,
            headless: None,
            size: (1600, 1000),
            split: None,
            split_size: None,
            scale: None,
            font_size: None,
            path: None,
        });
    }

    let mut cli = Cli {
        mode: Mode::Run,
        probe: false,
        dump_input: false,
        no_shm: false,
        crash: false,
        headless: None,
        size: (1600, 1000),
        split: None,
        split_size: None,
        scale: None,
        font_size: None,
        path: None,
    };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .ok_or_else(|| format!("{name} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--probe" => cli.probe = true,
            "--dump-input" => cli.dump_input = true,
            "--no-shm" => cli.no_shm = true,
            // Hidden: panic one second into the session to verify restoration.
            "--crash" => cli.crash = true,
            "--headless-frame" => cli.headless = Some(PathBuf::from(value("--headless-frame")?)),
            "--repo" => cli.path = Some(PathBuf::from(value("--repo")?)),
            "--split" => cli.split = Some(value("--split")?),
            "--size" => {
                let v = value("--size")?;
                if let Some((w, h)) = v.split_once('x') {
                    cli.size = (
                        w.parse().map_err(|_| format!("bad width {w:?}"))?,
                        h.parse().map_err(|_| format!("bad height {h:?}"))?,
                    );
                } else {
                    let f: f32 = v.parse().map_err(|_| format!("bad size {v:?}"))?;
                    if !(0.2..=0.95).contains(&f) {
                        return Err(format!("split size {f} out of range 0.2..0.95"));
                    }
                    cli.split_size = Some(f);
                }
            }
            "--scale" => {
                let v = value("--scale")?;
                let s: f32 = v.parse().map_err(|_| format!("bad scale {v:?}"))?;
                if !(0.5..=4.0).contains(&s) {
                    return Err(format!("scale {s} out of range 0.5..4"));
                }
                cli.scale = Some(s);
            }
            "--font-size" => {
                let v = value("--font-size")?;
                cli.font_size = Some(v.parse().map_err(|_| format!("bad font size {v:?}"))?);
            }
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other if other.starts_with('-') => {
                return Err(format!("unknown argument {other:?}\n{USAGE}"))
            }
            other => cli.path = Some(PathBuf::from(other)),
        }
    }
    Ok(cli)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cli, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_headless_and_size() {
        let c = p(&[
            "--headless-frame",
            "/tmp/x.png",
            "--size",
            "800x600",
            "--scale",
            "2",
        ])
        .unwrap();
        assert_eq!(c.headless.unwrap().to_str().unwrap(), "/tmp/x.png");
        assert_eq!(c.size, (800, 600));
        assert_eq!(c.scale, Some(2.0));
        assert!(!c.probe);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(p(&["--size", "800"]).is_err());
        assert!(p(&["--scale", "9"]).is_err());
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["--headless-frame"]).is_err());
    }

    #[test]
    fn positional_path() {
        let c = p(&["--no-shm", "/some/repo"]).unwrap();
        assert!(c.no_shm);
        assert_eq!(c.path.unwrap().to_str().unwrap(), "/some/repo");
    }

    #[test]
    fn parses_ls_and_action() {
        let c = p(&["ls"]).unwrap();
        assert!(matches!(c.mode, Mode::List));
        let c = p(&["action", r#"{"cmd":"status"}"#]).unwrap();
        match c.mode {
            Mode::Action { json, pid } => {
                assert_eq!(json, r#"{"cmd":"status"}"#);
                assert!(pid.is_none());
            }
            _ => panic!("expected action mode"),
        }
    }

    #[test]
    fn parses_split_and_size_fraction() {
        let c = p(&["--split", "right", "--size", "0.4"]).unwrap();
        assert_eq!(c.split.as_deref(), Some("right"));
        assert_eq!(c.split_size, Some(0.4));
    }
}
