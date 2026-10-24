// CamGlTest.qml — throwaway window for the TINHAT_CAM_GLTEST self-test (see cam_gltest.rs).
// Loaded by main.rs INSTEAD of Main.qml when TINHAT_CAM_GLTEST is set. It hosts a `vid_selfview`
// container (the C++ bridge `tinhat_video_attach` finds it by objectName and creates the
// GstGLQt6VideoItem inside it), kicks off the selfview pipeline after the window is realized,
// then exits 0 after a few seconds if no glupload crash occurred.
import QtQuick
import QtQuick.Window
import dev.tinhat.app

Window {
    id: win
    visible: true
    width: 960
    height: 540
    title: "Tin Hat — camera GL self-test"
    color: "#0a0a0a"

    CamGlTest { id: camGlTest }

    // The bridge attaches the qml6glsink's GstGLQt6VideoItem into this container by objectName.
    Item { objectName: "vid_selfview"; anchors.fill: parent }

    Text {
        anchors.centerIn: parent
        color: "#777777"
        text: "camera GL self-test — building selfview pipeline…"
    }

    // Give the window/scenegraph a moment to realize before attaching + PLAYING the pipeline.
    Timer { interval: 1500; running: true; repeat: false; onTriggered: camGlTest.start() }
    // If we get here without a SIGSEGV, the glupload path is safe → exit(0) = PASS.
    Timer { interval: 9000; running: true; repeat: false; onTriggered: camGlTest.finishOk() }
}
