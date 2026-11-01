# Generates hero.svg for the README: the Dwindle logo, its tiles lighting up in turn and its spiral
# drawing itself, beside a small Lituus window where a network connects. Works on light and dark pages.
import os, sys
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from logo import radius, spiral, split
out = sys.argv[1]
A = '#5cc8b0'
S=2.2; OX=38; OY=24
L=[]
L.append('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 840 330" width="840" height="330" role="img" aria-labelledby="t d">')
L.append('  <title id="t">Lituus</title>')
L.append('  <desc id="d">The Lituus logo, window tiles cut in a golden spiral with the spiral drawn through them, beside a Lituus window where a Wi-Fi network connects.</desc>')
L.append('''  <defs>
    <filter id="shadow" x="-20%" y="-20%" width="140%" height="150%">
      <feDropShadow dx="0" dy="12" stdDeviation="14" flood-color="#000" flood-opacity=".32"/>
    </filter>
    <radialGradient id="glow">
      <stop offset="0" stop-color="#5cc8b0" stop-opacity=".32"/>
      <stop offset="1" stop-color="#5cc8b0" stop-opacity="0"/>
    </radialGradient>
    <style>
      text { font-family: "Inter", "Cantarell", system-ui, -apple-system, "Segoe UI", sans-serif; }
      .float { transform-box: fill-box; transform-origin: center; animation: float 7s ease-in-out infinite; }
      @keyframes float { 0%, 100% { translate: 0 0; } 50% { translate: 0 -6px; } }
      .glow { transform-box: fill-box; transform-origin: center; animation: glow 3.5s ease-in-out infinite; }
      @keyframes glow { 0%, 100% { opacity: .55; scale: .92; } 50% { opacity: 1; scale: 1.06; } }
      .tile { animation: tile 7s ease-in-out infinite; }
      @keyframes tile { 0%, 100% { filter: none; } 8% { filter: brightness(1.7); } 20% { filter: none; } }
      .draw { stroke-dasharray: 1 1; animation: draw 7s ease-in-out infinite; }
      @keyframes draw { 0% { stroke-dashoffset: 1; } 45%, 100% { stroke-dashoffset: 0; } }
      /* The café row: available, then connecting, then connected, every 9 s. */
      .s0 { animation: s0 9s infinite; } @keyframes s0 { 0%, 22% { opacity: 1; } 26%, 96% { opacity: 0; } 100% { opacity: 1; } }
      .s1 { animation: s1 9s infinite; opacity: 0; } @keyframes s1 { 0%, 24% { opacity: 0; } 28%, 52% { opacity: 1; } 56%, 100% { opacity: 0; } }
      .s2 { animation: s2 9s infinite; opacity: 0; } @keyframes s2 { 0%, 54% { opacity: 0; } 58%, 94% { opacity: 1; } 98%, 100% { opacity: 0; } }
      .sel { animation: sel 9s infinite; opacity: 0; } @keyframes sel { 0%, 12% { opacity: 0; } 16%, 94% { opacity: 1; } 98%, 100% { opacity: 0; } }
      .spin { transform-box: fill-box; transform-origin: center; animation: spin 1s linear infinite; }
      @keyframes spin { to { rotate: 360deg; } }
      .bar { animation: bar 9s infinite; } @keyframes bar { 0%, 54% { fill: #3a3f46; } 58%, 94% { fill: #5cc8b0; } 98%, 100% { fill: #3a3f46; } }
      .scan { animation: scan 2.4s ease-in-out infinite; transform-box: fill-box; transform-origin: center; }
      @keyframes scan { 0%, 100% { rotate: 0deg; } 50% { rotate: 180deg; } }
      @media (prefers-reduced-motion: reduce) {
        .float, .glow, .tile, .draw, .spin, .scan, .bar, .s0, .s1, .s2, .sel { animation: none; }
        .s0, .s1 { opacity: 0; } .s2, .sel { opacity: 1; } .bar { fill: #5cc8b0; }
      }
    </style>
  </defs>''')
L.append('  <g class="float">')
L.append(f'    <circle class="glow" cx="{OX+64*S:.1f}" cy="{OY+64*S:.1f}" r="150" fill="url(#glow)"/>')
L.append(f'    <g transform="translate({OX} {OY}) scale({S})">')
L.append('      <rect x="4" y="4" width="120" height="120" rx="28" fill="#14171a" stroke="#262b30" stroke-width="1.2"/>')
tiles, arcs = split(108, 7)
shades = ["#24312e", "#29403a", "#2f4f47", "#376055", "#417466", "#4f9c89", "#5cc8b0"]
g = 2.5
for i, ((x, y, w, h), c) in enumerate(zip(tiles, shades)):
    L.append(f'      <rect class="tile" style="animation-delay: {i * 0.45:.2f}s" x="{x + g:.2f}" y="{y + g:.2f}" width="{w - 2 * g:.2f}" height="{h - 2 * g:.2f}" rx="{radius(w, h, 6):.2f}" fill="{c}"/>')
L.append(f'      <path class="draw" pathLength="1" d="{spiral(arcs)}" fill="none" stroke="#e9efe9" stroke-width="3.5" stroke-linecap="round"/>')
L.append('    </g>')
L.append('  </g>')
WX,WY,WW,WH=372,24,428,284
L.append(f'  <g filter="url(#shadow)"><rect x="{WX}" y="{WY}" width="{WW}" height="{WH}" rx="16" fill="#17191c" stroke="#2a2e33" stroke-width="1.5"/></g>')
L.append(f'  <text x="{WX+24}" y="{WY+40}" font-size="19" font-weight="700" fill="#e8eaed">Wi-Fi</text>')
L.append(f'  <g transform="translate({WX+WW-104} {WY+22})"><rect width="30" height="30" rx="8" fill="none" stroke="#353a40"/><g class="scan"><path d="M21 15a6 6 0 1 1-1.8-4.2M21 8.5v3h-3" fill="none" stroke="#c9ced4" stroke-width="1.6" stroke-linecap="round"/></g></g>')
L.append(f'  <rect x="{WX+WW-62}" y="{WY+26}" width="40" height="22" rx="11" fill="{A}"/><circle cx="{WX+WW-33}" cy="{WY+37}" r="8" fill="#0c1513"/>')
def bars(x,y,n,cls=''):
    s=''
    for i in range(4):
        h=4+i*3.5
        klass=f' class="{cls}"' if (cls and i>=n) else ''
        s+=f'<rect{klass} x="{x+i*5:.1f}" y="{y-h:.1f}" width="3.2" height="{h:.1f}" rx="1" fill="{A if i<n else "#3a3f46"}"/>'
    return s
def lock(x,y): return f'<path d="M{x} {y+5}h9v7h-9zM{x+2} {y+5}v-2a2.5 2.5 0 0 1 5 0v2" fill="none" stroke="#8b929a" stroke-width="1.3"/>'
rx=WX+14; rw=WW-28
L.append(f'  <text x="{WX+24}" y="{WY+80}" font-size="11" font-weight="700" letter-spacing=".8" fill="#8b929a">CONNECTED</text>')
y=WY+90
L.append(f'  <rect x="{rx}" y="{y}" width="{rw}" height="50" rx="9" fill="#1f2a27" stroke="#2c4a42"/>')
L.append('  '+bars(rx+14,y+33,4))
L.append(f'  <text x="{rx+50}" y="{y+22}" font-size="15" font-weight="600" fill="#e8eaed">Lumen Home</text>')
L.append(f'  <text x="{rx+50}" y="{y+40}" font-size="12.5" fill="{A}">Connected · WPA2/WPA3 · 5 GHz</text>')
L.append('  '+lock(rx+rw-26,y+18))
L.append(f'  <text x="{WX+24}" y="{WY+164}" font-size="11" font-weight="700" letter-spacing=".8" fill="#8b929a">AVAILABLE</text>')
y=WY+174
L.append(f'  <rect class="sel" x="{rx}" y="{y}" width="{rw}" height="50" rx="9" fill="#1f2a27" stroke="#2c4a42"/>')
L.append('  '+bars(rx+14,y+33,3,'bar'))
L.append(f'  <text x="{rx+50}" y="{y+22}" font-size="15" font-weight="600" fill="#e8eaed">Aster Café</text>')
L.append(f'  <text class="s0" x="{rx+50}" y="{y+40}" font-size="12.5" fill="#a0a7af">WPA2 · 2.4 / 5 GHz</text>')
L.append(f'  <g class="s1"><text x="{rx+66}" y="{y+40}" font-size="12.5" fill="{A}">Connecting…</text><circle class="spin" cx="{rx+56}" cy="{y+36}" r="5" fill="none" stroke="{A}" stroke-width="1.8" stroke-dasharray="20 12"/></g>')
L.append(f'  <text class="s2" x="{rx+50}" y="{y+40}" font-size="12.5" fill="{A}">Connected · password in your keyring</text>')
L.append('  '+lock(rx+rw-26,y+18))
y=WY+230
L.append(f'  <rect x="{rx}" y="{y}" width="{rw}" height="50" rx="9" fill="none"/>')
L.append('  '+bars(rx+14,y+33,2))
L.append(f'  <text x="{rx+50}" y="{y+22}" font-size="15" font-weight="600" fill="#e8eaed">Bibliothek-Free</text>')
L.append(f'  <text x="{rx+50}" y="{y+40}" font-size="12.5" fill="#a0a7af">Open · 5 GHz</text>')
L.append('</svg>')
open(out,'w').write('\n'.join(L)+'\n')
