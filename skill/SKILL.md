# gitgui agent skill

gitgui is a pixel-rendered git GUI that runs inside kitty-graphics terminals
(Ghostty, cmux, kitty). Use this skill when a gitgui instance is open in a
neighboring terminal pane and you need to inspect or drive the repository
through the GUI instead of raw git commands.

## Discover instances

```bash
gitgui ls
```

Lists running instances with pid, repo path, tty, and socket path.

## Send commands

```bash
gitgui action '{"cmd":"status"}'
gitgui action --pid 12345 '{"cmd":"select","oid":"abc123"}'
```

One JSON object per line. Responses are JSON:

```json
{"ok":true,"data":{...}}
{"ok":false,"error":"message"}
```

When run from the terminal pane that owns a gitgui instance, `action` connects
automatically via the controlling tty. Otherwise pass `--pid`.

## Commands

| Command | JSON | Notes |
|---|---|---|
| Status | `{"cmd":"status"}` | Branch, `head` oid, dirty counts, selection, `busy`, `last_op` |
| Select commit | `{"cmd":"select","oid":"abc123"}` | Prefix match on short or full oid; use `"working-tree"` for the index |
| Stage | `{"cmd":"stage","paths":["a.rs"]}` | Queues a stage on the worker thread |
| Unstage | `{"cmd":"unstage","paths":["a.rs"]}` | Queues an unstage |
| Commit | `{"cmd":"commit","message":"..."}` | Staged files only |
| Commit and push | `{"cmd":"commit_and_push","message":"...","amend":false}` | Commit then `git push` |
| Fetch | `{"cmd":"fetch"}` | Opens network log |
| Pull | `{"cmd":"pull"}` | Opens network log |
| Push | `{"cmd":"push"}` | Opens network log |
| Result | `{"cmd":"result","id":"req-7"}` | Outcome of a write sent with that `id` |
| Screenshot | `{"cmd":"screenshot","path":"/tmp/frame.png"}` | Saves the current frame as PNG |
| List | `{"cmd":"list"}` | Same as `gitgui ls` |

Write operations return `{"queued":"..."}` immediately; poll `status` until
`busy` is zero, then read `last_op` (`{"label","ok","message"}`) and `head`.

Retries: every write accepts an optional `id` (any string you pick). If the
response is lost or you time out, send the same command with the same `id`:
gitgui does not queue it again, it answers with what happened to the first
one, `{"id":..,"state":"queued"|"done","ok":..,"result":..,"duplicate":true}`.
`{"cmd":"result","id":..}` reads the same record. Without an `id` a retry is a
new write; a second `commit` with nothing staged fails with
"nothing to commit" rather than creating an empty commit.

```bash
gitgui action '{"cmd":"commit_and_push","message":"fix layout","id":"cp-1"}'
gitgui action '{"cmd":"result","id":"cp-1"}'
```

## Open in a split

```bash
gitgui --split right .
gitgui --split down /path/to/repo
```

In cmux this uses `cmux new-split` and `cmux send`. In kitty it uses
`kitty @ launch`. Ghostty prints a keybinding hint when no CLI split is
available.

## Headless frame (no running instance)

```bash
gitgui --headless-frame /tmp/frame.png --repo .
```

Renders one PNG without a terminal. Useful for CI and visual regression.
