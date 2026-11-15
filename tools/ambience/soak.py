#!/usr/bin/env python3
# soak.py — 10 minutes of an effect with the rows churning; the app's memory (RSS of its whole process tree) every 30 s.
#   desktop: soak.py desktop BINARY OUT.json [secs]   (TINHAT_AMBIENCE_GALLERY --run … --soak, forced on, Xvfb; a LAB
#            build: a release binary has neither switch)
#   web:     soak.py web URLBASE OUT.json [secs]       (#/__gallery/ambience?soak=1&force=1 in Chromium + its JS heap)
import json, os, subprocess, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
mode, target, out = sys.argv[1], sys.argv[2], sys.argv[3]
secs = int(sys.argv[4]) if len(sys.argv) > 4 else 600

def tree(pid):
    seen, todo = set(), [pid]
    while todo:
        p = todo.pop(); seen.add(p)
        try:
            for t in os.listdir(f"/proc/{p}/task"):
                try:
                    todo += [int(k) for k in open(f"/proc/{p}/task/{t}/children").read().split() if int(k) not in seen]
                except OSError: pass
        except OSError: pass
    return seen

def rss(ps):
    s = 0
    for p in ps:
        try:
            for l in open(f"/proc/{p}/status"):
                if l.startswith("VmRSS:"): s += int(l.split()[1])
        except OSError: pass
    return s // 1024

if mode == "desktop":
    env = dict(os.environ, HOME="/tmp/ambience-home", TINHAT_AMBIENCE_GALLERY="1", TINHAT_GUI_THEMESYNC="0", QT_QPA_PLATFORM="xcb", TINHAT_AMBIENCE_FORCE="1", TINHAT_NO_SINGLE_INSTANCE="1")
    p = subprocess.Popen(["xvfb-run", "-a", "-s", "-screen 0 1280x800x24", target, f"--run=rain:oled:{secs + 30}", "--soak", "--size=1100x720"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
else:
    js = f"""
const {{ chromium }} = require('playwright');
(async () => {{
  const b = await chromium.launch({{ args: ['--enable-unsafe-swiftshader', '--ignore-gpu-blocklist', '--enable-precise-memory-info'] }});
  const pg = await (await b.newContext({{ viewport: {{ width: 1100, height: 720 }} }})).newPage();
  await pg.goto('{target}/#/__gallery/ambience?preset=rain&theme=oled&seed=3&soak=1&force=1');
  await pg.waitForSelector('[data-gallery-ready]');
  for (let i = 0; i <= {secs // 30}; i++) {{
    const h = await pg.evaluate(() => {{ const m = performance.memory; return m ? Math.round(m.usedJSHeapSize / 1048576 * 10) / 10 : -1; }});
    const st = await pg.evaluate(() => document.querySelector('[data-ambience]')?.getAttribute('data-ambience-state'));
    console.log('HEAP ' + i * 30 + ' ' + h + ' ' + st);
    if (i < {secs // 30}) await pg.waitForTimeout(30000);
  }}
  await b.close();
}})();
"""
    open("/tmp/ambience-soak.cjs", "w").write(js)
    p = subprocess.Popen(["node", "/tmp/ambience-soak.cjs"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True, env=dict(os.environ, NODE_PATH=os.path.join(os.getcwd(), "node_modules")), text=True)
samples = []
t0 = time.time()
time.sleep(10)
while time.time() - t0 < secs + 5:
    samples.append({"t": int(time.time() - t0), "rss_mb": rss(tree(p.pid))})
    print(json.dumps(samples[-1]), flush=True)
    time.sleep(30)
heap = []
if mode == "web":
    o = p.communicate(timeout=120)[0]
    heap = [l for l in o.splitlines() if l.startswith("HEAP")]
    print("\n".join(heap))
else:
    p.wait(timeout=120)
json.dump({"rss": samples, "heap": heap}, open(out, "w"), indent=1)
