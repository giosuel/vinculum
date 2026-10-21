# Generates hero.svg for the README: the node-cube logo with signals running along its edges,
# beside a small Vinculum window where a network connects. Works on light and dark pages.
import sys
out = sys.argv[1]
A = '#5cc8b0'; DIM = '#3c5650'
top=(32,8); ul=(11.22,20); ur=(52.78,20); hub=(32,32); ll=(11.22,44); lr=(52.78,44); bot=(32,56)
front=[(top,ul),(top,ur),(ll,ul),(ll,bot),(ul,hub),(lr,ur),(lr,bot),(ur,hub),(bot,hub)]
spokes=[(hub,top),(hub,ll),(hub,lr)]
S=4.1; OX=48; OY=34
def P(p): return (OX+p[0]*S, OY+p[1]*S)
L=[]
L.append('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 840 330" width="840" height="330" role="img" aria-labelledby="t d">')
L.append('  <title id="t">Vinculum</title>')
L.append('  <desc id="d">The Vinculum logo, a cube of linked nodes with signals running along its edges, beside a Vinculum window where a Wi-Fi network connects.</desc>')
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
      .node { animation: node 3.5s ease-in-out infinite; }
      @keyframes node { 0%, 100% { fill: #5cc8b0; } 45% { fill: #c9f3e8; } 60% { fill: #5cc8b0; } }
      .n1 { animation-delay: -.5s; } .n2 { animation-delay: -1s; } .n3 { animation-delay: -1.5s; }
      .n4 { animation-delay: -2s; } .n5 { animation-delay: -2.5s; } .n6 { animation-delay: -3s; }
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
        .float, .glow, .node, .spin, .scan, .bar, .s0, .s1, .s2, .sel { animation: none; }
        .s0, .s1 { opacity: 0; } .s2, .sel { opacity: 1; } .bar { fill: #5cc8b0; }
      }
    </style>
  </defs>''')
L.append('  <g class="float">')
cx,cy=P(hub)
L.append(f'    <circle class="glow" cx="{cx:.1f}" cy="{cy:.1f}" r="150" fill="url(#glow)"/>')
for a,b in spokes:
    (x1,y1),(x2,y2)=P(a),P(b)
    L.append(f'    <line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" stroke="{DIM}" stroke-width="5" stroke-linecap="round" opacity=".8"/>')
for a,b in front:
    (x1,y1),(x2,y2)=P(a),P(b)
    L.append(f'    <line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" stroke="{A}" stroke-width="8.5" stroke-linecap="round"/>')
loops=[[top,ur,lr,bot,ll,ul,top],[ul,hub,lr,ur,hub,bot,ll,ul],[top,ul,hub,ur,top]]
for i,loop in enumerate(loops):
    d='M'+' L'.join(f'{x:.1f} {y:.1f}' for x,y in (P(p) for p in loop))
    dur=[6,7.5,5][i]
    L.append(f'    <circle r="14" fill="{A}" opacity=".3"><animateMotion dur="{dur}s" repeatCount="indefinite" path="{d}"/></circle>')
    L.append(f'    <circle r="6.5" fill="#f2fffb"><animateMotion dur="{dur}s" repeatCount="indefinite" path="{d}"/></circle>')
for i,n in enumerate([top,ul,ur,ll,lr,bot]):
    x,y=P(n)
    L.append(f'    <circle class="node n{i+1}" cx="{x:.1f}" cy="{y:.1f}" r="13" fill="{A}"/>')
L.append(f'    <circle cx="{cx:.1f}" cy="{cy:.1f}" r="13.5" fill="#a6e6d6"/>')
L.append(f'    <circle cx="{cx:.1f}" cy="{cy:.1f}" r="20" fill="none" stroke="{A}" stroke-width="2.5" opacity=".55"/>')
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
