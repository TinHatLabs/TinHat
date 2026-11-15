#!/usr/bin/env python3
# perf.py — CPU cost of chat backgrounds + particle effects (docs/AMBIENCE.md): per scene, the CPU time of the app's whole
# process tree (every thread, the renderer's included) over a steady window, as % of ONE core; and frames/s.
#   desktop: perf.py desktop BINARY OUT.json [seconds]     (TINHAT_AMBIENCE_GALLERY --run, under Xvfb; BINARY = a LAB build:
#            a release binary has neither the gallery nor TINHAT_AMBIENCE_FORCE)
#   web:     perf.py web URLBASE OUT.json [seconds]         (Playwright Chromium on #/__gallery/ambience)
import json, os, subprocess, sys, time

HZ = os.sysconf("SC_CLK_TCK")
# a real GPU in headless Chromium (Linux: ANGLE on EGL through Mesa)
GPU_ARGS = ["--use-angle=gl-egl", "--ignore-gpu-blocklist", "--enable-gpu-rasterization", "--enable-zero-copy"]

def tree(pid):
    out, todo = set(), [pid]
    while todo:
        p = todo.pop()
        out.add(p)
        try:
            for t in os.listdir(f"/proc/{p}/task"):
                try:
                    kids = open(f"/proc/{p}/task/{t}/children").read().split()
                    todo += [int(k) for k in kids if int(k) not in out]
                except OSError:
                    pass
        except OSError:
            pass
    return out

def cpu(pids):
    s = 0
    for p in pids:
        try:
            f = open(f"/proc/{p}/stat").read().rsplit(")", 1)[1].split()
            s += int(f[11]) + int(f[12])
        except OSError:
            pass
    return s / HZ

def rss(pids):
    s = 0
    for p in pids:
        try:
            for l in open(f"/proc/{p}/status"):
                if l.startswith("VmRSS:"):
                    s += int(l.split()[1])
        except OSError:
            pass
    return s // 1024

def window(root, warm, secs):
    time.sleep(warm)
    ps = tree(root)
    c0, t0 = cpu(ps), time.time()
    time.sleep(secs)
    ps = tree(root)
    c1, t1 = cpu(ps), time.time()
    return round(100 * (c1 - c0) / (t1 - t0), 1), rss(ps)

mode, target, out = sys.argv[1], sys.argv[2], sys.argv[3]
secs = int(sys.argv[4]) if len(sys.argv) > 4 else 30
scenes = [("none", "oled"), ("dusk", "oled"), ("rain", "oled"), ("hearth", "oled"), ("snow", "oled"), ("summer", "oled"), ("rain", "atom-one-light")]
res = []
for preset, theme in scenes:
    if mode == "desktop":
        env = dict(os.environ, HOME="/tmp/ambience-home", TINHAT_AMBIENCE_GALLERY="1", TINHAT_GUI_THEMESYNC="0", QT_QPA_PLATFORM="xcb", TINHAT_FRAME_METER="1", TINHAT_AMBIENCE_FORCE=os.environ.get("FORCE", "1"), TINHAT_NO_SINGLE_INSTANCE="1")
        log = f"/tmp/ambience-perf-{preset}-{theme}.log"
        p = subprocess.Popen(["xvfb-run", "-a", "-s", "-screen 0 1280x800x24", target, f"--run={preset}:{theme}:{secs + 12}", "--size=1100x720"], env=env, stdout=open(log, "w"), stderr=subprocess.STDOUT, start_new_session=True)
        pct, mem = window(p.pid, 8, secs)
        p.wait(timeout=60)
        frames = [l for l in open("/tmp/ambience-home/.local/state/tinhat/tinhat.log") if "AMBIENCE frames" in l][-3:]
    else:
        js = f"""
const {{ chromium }} = require('playwright');
(async () => {{
  const b = await chromium.launch({{ args: {json.dumps(GPU_ARGS if os.environ.get("GPU") == "1" else ["--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"])} }});
  const ctx = await b.newContext({{ viewport: {{ width: 1100, height: 720 }} }});
  const pg = await ctx.newPage();
  await pg.goto('{target}/#/__gallery/ambience?preset={preset}&theme={theme}&seed=3{"&force=1" if os.environ.get("FORCE", "1") == "1" else ""}');
  await pg.waitForSelector('[data-gallery-ready]');
  await pg.evaluate(() => {{ window.__f = 0; const t = () => {{ window.__f++; requestAnimationFrame(t) }}; requestAnimationFrame(t) }});
  await pg.waitForTimeout({(secs + 8) * 1000});
  const f = await pg.evaluate(() => window.__f);
  const info = await pg.evaluate(() => {{ const c = document.createElement('canvas').getContext('webgl2'); if (!c) return 'no webgl2'; const d = c.getExtension('WEBGL_debug_renderer_info'); return String(c.getParameter(d ? d.UNMASKED_RENDERER_WEBGL : c.RENDERER)); }});
  const st = await pg.evaluate(() => document.querySelector('[data-ambience]')?.getAttribute('data-ambience-state') ?? 'none');
  console.log('RAF ' + f + ' state ' + st + ' renderer ' + info);
  await b.close();
}})();
"""
        open("/tmp/ambience-perf.cjs", "w").write(js)
        p = subprocess.Popen(["node", "/tmp/ambience-perf.cjs"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True, cwd=os.getcwd(), env=dict(os.environ, NODE_PATH=os.path.join(os.getcwd(), "node_modules")))
        pct, mem = window(p.pid, 8, secs)
        o = p.communicate(timeout=120)[0].decode()
        frames = [l for l in o.splitlines() if l.startswith("RAF")]
    r = {"preset": preset, "theme": theme, "cpu_pct_one_core": pct, "rss_mb": mem, "frames": [f.strip()[-90:] for f in frames]}
    print(json.dumps(r), flush=True)
    res.append(r)
json.dump(res, open(out, "w"), indent=1)
