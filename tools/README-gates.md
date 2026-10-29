# Gate notes (tools/)

## Declined rings (DM ringing, ring glue — 12.1.1)
"<name> declined" is a passive notification for `DECLINED_SHOWN_MS` = **6000 ms** (`crates/app/src/ringglue.rs`; the
web's `ringglue.ts DECLINED_SHOWN_MS`; both checked against `declinedShownMs` in `testvectors/ringdecline.json`). A
gate that samples the screen can miss it, so assert the stderr line instead (one per decline; it proves the decline was
taken and the notice requested, not that it was painted; match the ring id of THIS call):

    [ringglue] ring in <roomId> declined by <decliner mxid> (ring <ring id>) — notice requested: "<name> declined" for 6000 ms

The web's equivalents (its tools/README-gates.md): `<html data-th-declined="<roomId> <mxid> <ring id> <ms>">` and the
console line `[callwatch] ring in <roomId> declined by <mxid> (ring <ring id>)`.
