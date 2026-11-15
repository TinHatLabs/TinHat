//! AmbienceItem — the chat's particle layer on the desktop (docs/AMBIENCE.md; the web's `ambience/layer.tsx` + `gl.ts`).
//!
//! The simulation and its mesh are `tinhat_widgets::ambience` (the web's `core.ts`, proven identical by the shared
//! vectors); this item only owns one sim, takes the panels (message bubbles, day pills, names, reactions) from QML as
//! JSON, steps on QML's FrameAnimation (`tick`), and hands the mesh to ONE scene-graph node (`ambience_bridge.h`:
//! QSGVertexColorMaterial, no custom shader). It sits behind the message list; the mesh never draws inside a panel.

use core::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use tinhat_widgets::ambience::{AmbienceSim, Collider, Effect, MeshOut, Mode, WARM_S};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    unsafe extern "C++" {
        include!(<QtQuick/QQuickItem>);
        /// Base class: a plain item that draws its own scene-graph node.
        type QQuickItem;
        include!("ambience_bridge.h");
        type QSGNode;
        type QQuickItemUpdatePaintNodeData;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QQuickItem]
        #[qproperty(QString, effect)] //  QML: effect — "rain" | "embers" | "snow" | "fireflies" | "none"
        #[qproperty(QString, mode)] //    QML: mode — "dark" | "oled" | "light" (core themeMode of the timeline background)
        #[qproperty(f64, density)] //     QML: density — 1 (0.6 on small windows, as the web's phones)
        #[qproperty(i32, verts)] //       QML: verts — vertices in the last frame (logs, tests)
        #[qproperty(i32, particles)] //   QML: particles — pieces alive (logs, tests)
        #[qproperty(bool, cpu_device)] // QML: cpuDevice — the window is drawn by a CPU rasteriser: no particles there
                                       //      (lab builds: TINHAT_AMBIENCE_FORCE=1 ignores it, the worst-case measurements)
        type AmbienceItem = super::AmbienceItemRust;

        /// QQuickItem::updatePaintNode — the mesh into one geometry node (render thread, GUI thread blocked).
        #[cxx_override]
        #[cxx_name = "updatePaintNode"]
        unsafe fn update_paint_node(self: Pin<&mut AmbienceItem>, old: *mut QSGNode, data: *mut QQuickItemUpdatePaintNodeData) -> *mut QSGNode;

        #[inherit]
        fn update(self: Pin<&mut AmbienceItem>);
        #[inherit]
        fn width(self: &AmbienceItem) -> f64;
        #[inherit]
        fn height(self: &AmbienceItem) -> f64;

        /// QML: setColliders(json) — the panels in view, in this item's coordinates:
        /// `[{id, x, y, w, h, rl, rr}]` (rl / rr = the top corners' radii).
        #[qinvokable]
        fn set_colliders(self: Pin<&mut AmbienceItem>, json: &QString);
        /// QML: tick(seconds) — one frame: step the sim by the frame's time (fixed steps), rebuild the mesh, repaint.
        #[qinvokable]
        fn tick(self: Pin<&mut AmbienceItem>, dt: f64);
        /// QML: clear() — drop the particles and draw nothing (effects off).
        #[qinvokable]
        fn clear(self: Pin<&mut AmbienceItem>);
    }

    unsafe extern "C++" {
        #[cxx_name = "tinhatAmbienceInit"]
        fn ambience_init(item: Pin<&mut AmbienceItem>);
        #[cxx_name = "tinhatAmbienceCpuDevice"]
        fn ambience_cpu_device(item: Pin<&mut AmbienceItem>) -> bool;
        #[cxx_name = "tinhatAmbienceNode"]
        unsafe fn ambience_node(item: Pin<&mut AmbienceItem>, old: *mut QSGNode, v: *const f32, n: usize) -> *mut QSGNode;
    }

    impl cxx_qt::Initialize for AmbienceItem {}
}

pub struct AmbienceItemRust {
    effect: QString,
    mode: QString,
    density: f64,
    verts: i32,
    particles: i32,
    cpu_device: bool,
    /// what the render thread saw (updatePaintNode); published to `cpu_device` on the next tick
    cpu_seen: bool,
    sim: Option<AmbienceSim>,
    sim_effect: String,
    cols: Vec<Collider>,
    cols_dirty: bool,
    view: (f64, f64),
    warmed: bool,
    mesh: MeshOut,
    /// what the render thread draws (copied from `mesh` at the end of a tick)
    draw: Vec<f32>,
}

impl Default for AmbienceItemRust {
    fn default() -> Self {
        AmbienceItemRust {
            effect: QString::from("none"),
            mode: QString::from("dark"),
            density: 1.0,
            verts: 0,
            particles: 0,
            cpu_device: false,
            cpu_seen: false,
            sim: None,
            sim_effect: String::new(),
            cols: Vec::new(),
            cols_dirty: true,
            view: (0.0, 0.0),
            warmed: false,
            mesh: MeshOut::new(4096),
            draw: Vec::new(),
        }
    }
}

impl cxx_qt::Initialize for qobject::AmbienceItem {
    fn initialize(self: Pin<&mut Self>) {
        qobject::ambience_init(self);
    }
}

fn parse_colliders(json: &str) -> Vec<Collider> {
    let Ok(serde_json::Value::Array(a)) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let num = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    a.iter()
        .filter_map(|v| {
            let id = v.get("id")?.as_str()?.to_string();
            let (w, h) = (num(v, "w"), num(v, "h"));
            if !(w >= 2.0 && h >= 2.0) {
                return None;
            }
            Some(Collider { id, x: num(v, "x"), y: num(v, "y"), w, h, rl: Some(num(v, "rl")), rr: Some(num(v, "rr")) })
        })
        .collect()
}

impl qobject::AmbienceItem {
    fn set_colliders(mut self: Pin<&mut Self>, json: &QString) {
        let cols = parse_colliders(&json.to_string());
        let mut r = self.as_mut().rust_mut();
        r.cols = cols;
        r.cols_dirty = true;
    }

    fn tick(mut self: Pin<&mut Self>, dt: f64) {
        let (w, h) = (self.width(), self.height());
        let effect = self.effect().to_string();
        let mode = Mode::parse(&self.mode().to_string()).unwrap_or(Mode::Dark);
        let density = *self.density();
        let (n, alive) = {
            let mut r = self.as_mut().rust_mut();
            let r = &mut *r;
            let Some(eff) = Effect::parse(&effect).filter(|e| *e != Effect::None) else {
                r.sim = None;
                r.draw.clear();
                return;
            };
            if r.sim.is_none() || r.sim_effect != effect {
                let seed = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(1) & 0x7fff_ffff) as u32;
                r.sim = Some(AmbienceSim::new(eff, seed, if density > 0.0 { density } else { 1.0 }));
                r.sim_effect = effect.clone();
                r.warmed = false;
                r.cols_dirty = true;
                r.view = (-1.0, -1.0); // a new sim learns the view size below
                eprintln!("[ambience] effect {effect} on (density {density})");
            }
            let sim = r.sim.as_mut().unwrap();
            if r.view != (w, h) {
                r.view = (w, h);
                sim.set_view(w, h);
            }
            if r.cols_dirty {
                r.cols_dirty = false;
                sim.set_colliders(&r.cols);
            }
            if !r.warmed {
                r.warmed = true;
                sim.warm(WARM_S);
            }
            sim.advance(dt);
            let n = sim.mesh(mode, &mut r.mesh);
            r.draw.clear();
            r.draw.extend_from_slice(&r.mesh.buf[..n * 6]);
            (n as i32, sim.parts.len() as i32)
        };
        self.as_mut().set_verts(n);
        self.as_mut().set_particles(alive);
        let cpu = self.as_ref().rust().cpu_seen && !ambience_forced();
        if cpu != *self.cpu_device() {
            if cpu {
                eprintln!("[ambience] drawn by a CPU rasteriser: particles off (the still background stays)");
            }
            self.as_mut().set_cpu_device(cpu);
        }
        self.update();
    }

    fn clear(mut self: Pin<&mut Self>) {
        {
            let mut r = self.as_mut().rust_mut();
            r.sim = None;
            r.sim_effect.clear();
            r.draw.clear();
        }
        self.as_mut().set_verts(0);
        self.as_mut().set_particles(0);
        self.update();
    }

    /// # Safety
    /// Called by Qt's scene graph with the item's old node (or null); returns the node to keep (or null).
    unsafe fn update_paint_node(mut self: Pin<&mut Self>, old: *mut qobject::QSGNode, _data: *mut qobject::QQuickItemUpdatePaintNodeData) -> *mut qobject::QSGNode {
        let (ptr, len) = {
            let this = self.as_ref();
            let r = this.rust();
            (r.draw.as_ptr(), r.draw.len())
        };
        let cpu = qobject::ambience_cpu_device(self.as_mut());
        self.as_mut().rust_mut().cpu_seen = cpu;
        // SAFETY: `draw` is not touched while the GUI thread is blocked for this sync; Qt owns the returned node.
        unsafe { qobject::ambience_node(self.as_mut(), old, ptr, len) }
    }
}


/// The worst-case measurement switch (G3's "effects forced on" runs): particles even on a CPU rasteriser. A lab hook
/// like the self-tests: compiled only outside a release (cfg tinhat_env_selftests, build.rs; a release build sets
/// TINHAT_RELEASE_BUILD=1), and scripts/release-binary-check.sh refuses a binary that still names it.
#[cfg(tinhat_env_selftests)]
fn ambience_forced() -> bool {
    std::env::var_os("TINHAT_AMBIENCE_FORCE").is_some()
}
#[cfg(not(tinhat_env_selftests))]
fn ambience_forced() -> bool {
    false
}

#[cfg(test)]
mod force_switch_tests {
    #[test]
    fn ambience_force_is_a_lab_switch() {
        // lab builds (tests are built with tinhat_env_selftests) honour it …
        #[cfg(tinhat_env_selftests)]
        {
            std::env::set_var("TINHAT_AMBIENCE_FORCE", "1");
            assert!(super::ambience_forced());
            std::env::remove_var("TINHAT_AMBIENCE_FORCE");
            assert!(!super::ambience_forced());
        }
        // … and the switch is read in one place only, inside the lab-only function (a release build can't name it)
        let src = include_str!("ambience_item.rs");
        let code: String = src.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        let reads: Vec<_> = code.match_indices(concat!("\"TINHAT_AMBIENCE", "_FORCE\"")).collect();
        assert_eq!(reads.len(), 3, "the reader + this test's set_var / remove_var only");
        let at = code.find(concat!("var_os(\"TINHAT_AMBIENCE", "_FORCE\")")).expect("the reader");
        let gate = code[..at].rfind("#[cfg(").expect("its cfg");
        assert!(code[gate..at].starts_with("#[cfg(tinhat_env_selftests)]"), "the reader sits behind cfg(tinhat_env_selftests)");
    }
}
