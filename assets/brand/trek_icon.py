import math, sys
# Trek mark: a single tapered switchback ribbon climbing into perspective, ending at a summit beacon.
levels=[(792,258),(662,196),(550,142),(458,98),(386,62)]   # (y, half-width)
W0,W1=66,26  # stroke width at bottom / top
def cubic(p0,p1,p2,p3,n=40):
    return [tuple((1-t)**3*a+3*(1-t)**2*t*b+3*(1-t)*t*t*c+t**3*d for a,b,c,d in zip(p0,p1,p2,p3)) for t in (i/n for i in range(n+1))]
def line(p0,p1,n=30): return [(p0[0]+(p1[0]-p0[0])*i/n, p0[1]+(p1[1]-p0[1])*i/n) for i in range(n+1)]
pts=[]
for i in range(len(levels)-1):
    (y0,h0),(y1,h1)=levels[i],levels[i+1]; s=1 if i%2==0 else -1
    xs,xe=512-s*h0,512+s*h0; r=(y0-y1)/2
    pts+=line((xs,y0),(xe,y0))[:-1]
    pts+=cubic((xe,y0),(xe+s*r*1.3,y0),(512+s*h1+s*r*1.3,y1),(512+s*h1,y1))[:-1]
yT,hT=levels[-1]; s=1 if (len(levels)-1)%2==0 else -1
pts+=line((512-s*hT,yT),(512-0.0,yT))[:-1]
# soft corner into the final rise
pts+=cubic((512-s*24,yT),(512,yT),(512,yT),(512,yT-24),12)[:-1]
pts+=line((512,yT-24),(512,yT-62))
# dedupe
P=[pts[0]]
for p in pts[1:]:
    if math.dist(p,P[-1])>0.5: P.append(p)
ys=[p[1] for p in P]; ymax,ymin=max(ys),min(ys)
def width(y): t=(ymax-y)/(ymax-ymin); return W0+(W1-W0)*(t**0.85)
L=[];R=[]
for i,p in enumerate(P):
    a=P[max(i-1,0)]; b=P[min(i+1,len(P)-1)]
    dx,dy=b[0]-a[0],b[1]-a[1]; n=math.hypot(dx,dy); nx,ny=-dy/n,dx/n; w=width(p[1])/2
    L.append((p[0]+nx*w,p[1]+ny*w)); R.append((p[0]-nx*w,p[1]-ny*w))
f=lambda q:f"{q[0]:.1f} {q[1]:.1f}"
ribbon="M"+" L".join(f(q) for q in L)+" L"+" L".join(f(q) for q in reversed(R))+"Z"
cap0=(P[0],width(P[0][1])/2); cap1=(P[-1],width(P[-1][1])/2)
def mark(fill):
    return (f'<path d="{ribbon}" fill="{fill}"/>'
            f'<circle cx="{cap0[0][0]:.1f}" cy="{cap0[0][1]:.1f}" r="{cap0[1]:.1f}" fill="{fill}"/>'
            f'<circle cx="{cap1[0][0]:.1f}" cy="{cap1[0][1]:.1f}" r="{cap1[1]:.1f}" fill="{fill}"/>')
BEACON_Y=262
defs='''<defs>
<linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{b0}"/><stop offset="1" stop-color="{b1}"/></linearGradient>
<linearGradient id="sun" gradientUnits="userSpaceOnUse" x1="250" y1="800" x2="600" y2="300"><stop offset="0" stop-color="#FF4D2E"/><stop offset="0.55" stop-color="#FF8A3D"/><stop offset="1" stop-color="#FFC56B"/></linearGradient>
<radialGradient id="glow"><stop offset="0" stop-color="#FFC56B" stop-opacity="0.85"/><stop offset="0.45" stop-color="#FF8A3D" stop-opacity="0.25"/><stop offset="1" stop-color="#FF8A3D" stop-opacity="0"/></radialGradient>
<linearGradient id="rim" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="{rim}"/><stop offset="0.35" stop-color="#fff" stop-opacity="0"/></linearGradient>
</defs>'''
def icon(b0,b1,fill,beacon,rim="0.16"):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'+defs.format(b0=b0,b1=b1,rim=rim)+
      '<rect x="100" y="100" width="824" height="824" rx="186" fill="url(#bg)"/>'
      '<rect x="101.5" y="101.5" width="821" height="821" rx="184.5" fill="none" stroke="url(#rim)" stroke-width="3"/>'
      +mark(fill)+f'<circle cx="512" cy="{BEACON_Y}" r="118" fill="url(#glow)"/><circle cx="512" cy="{BEACON_Y}" r="34" fill="{beacon}"/></svg>')
open("trek-icon-dark.svg","w").write(icon("#1D2028","#0A0B0E","url(#sun)","#FFF1D6"))
open("trek-icon-light.svg","w").write(icon("#FAF6EF","#E9E1D3","#17191F","#FF6A2B","0.6").replace('r="34" fill="#FF6A2B"/>','r="34" fill="#FF6A2B" stroke="#17191F" stroke-width="10"/>'))
open("trek-icon-mono.svg","w").write(icon("#1D2028","#0A0B0E","#E8ECF3","#FFF1D6"))
# glyph-only mark (for in-app logo, onboarding animation): transparent, 1024 viewbox
open("trek-mark.svg","w").write(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="140 200 744 640">'+defs.format(b0="#000",b1="#000",rim="0")+mark("url(#sun)")+f'<circle cx="512" cy="{BEACON_Y}" r="34" fill="#FFC56B"/></svg>')
# menu-bar template glyph: monochrome, black on transparent, simplified (3 switchbacks), 22pt grid
open("centerline.txt","w").write("\n".join(f"{x:.2f},{y:.2f}" for x,y in P))
