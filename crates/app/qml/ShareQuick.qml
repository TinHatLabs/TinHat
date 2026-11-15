// ShareQuick — the share picker's "Custom…" panel (12.1.15): which rows of the app's own lists the current share
// quality is (resolution, frame rate, bandwidth), and the quality a pick gives. Logic only; StreamPopover.qml draws
// the panel. A quality is [width, height, fps, kbps]. The rules are Tin Hat Web's (src/features/callui/shareQuick.ts);
// both sides run testvectors/share-custom-quick/vectors.json (here: scripts/share-custom-quick-test.qml, run by
// scripts/qml-lint.sh).
import QtQml

QtObject {
    id: quick
    property var resList: []                       // [{ label, w, h, base }]: AppCore.shareResList
    property var bwList: []                        // [{ label, f }]: AppCore.shareBwList
    readonly property var rates: [5, 15, 30, 60]
    readonly property var rateLabels: rates.map(function (r) { return r + " fps" })

    // the row with exactly that size, else 1080p (AppCore.shareCurRes)
    function resIndex(q) {
        for (var i = 0; i < resList.length; i++)
            if (resList[i].w === q[0] && resList[i].h === q[1]) return i
        return 2
    }
    // that rate, else 30 fps
    function fpsIndex(q) {
        var i = rates.indexOf(q[2])
        return i >= 0 ? i : rates.indexOf(30)
    }
    // the step whose bitrate for the current resolution row is nearest; the lower step on a tie (AppCore.shareCurTier)
    function bwIndex(q) {
        var base = resList[resIndex(q)].base
        var best = 1, bestD = 1e12
        for (var i = 0; i < bwList.length; i++) {
            var d = Math.abs(q[3] - Math.round(base * bwList[i].f))
            if (d < bestD) { bestD = d; best = i }
        }
        return best
    }
    // the quality after picking row `i` of "res" | "fps" | "bw": a resolution keeps the frame rate and the bandwidth
    // STEP, a frame rate keeps size and bitrate, a bandwidth step keeps size and frame rate
    function pick(q, what, i) {
        if (what === "res") return [resList[i].w, resList[i].h, q[2], Math.round(resList[i].base * bwList[bwIndex(q)].f)]
        if (what === "fps") return [q[0], q[1], rates[i], q[3]]
        return [q[0], q[1], q[2], Math.round(resList[resIndex(q)].base * bwList[i].f)]
    }
}
