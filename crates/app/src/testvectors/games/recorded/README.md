# Recorded from a real lab server (real-shaped fixtures)

`lab.jsonl`: what a real lab play server sent the desktop app on 2026-11-19 (fenced `TINHAT_PLAY_RECORD` hook, a
debug build only), one JSON line each: `{"kind": "http"|"frame", "what": "<METHOD path>"|"<frame type>", "body": "<raw>"}`.
Never the session exchange, never `hello`/`auth`. Names are neutralised (server → `lab.example.org`, people →
`@playerN`); nothing else is changed, so the shapes (arrays vs objects, nulls, number types) are the server's own.
`play_ctl::tests::recorded_lab_shapes` runs every line through the app's own checks (launch-gate 2026-11-19 13:23).
