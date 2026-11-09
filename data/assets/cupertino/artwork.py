#!/usr/bin/env python3
"""Original top-lit glass app tiles and free-standing system objects."""

import math

# Master artwork has room for the light cast below every object.
CANVAS = 1024
# App tiles occupy the shared 824-unit keyline.
TILE_SIZE = 824
# The fourth-power boundary blends the straight edges into the corners.
TILE_POWER = 4.2
# All artwork is lit from a broad source above and slightly to the left.
LIGHT_POSITION = (340, 120)


def path(d, fill, attrs=""):
    return f'<path d="{d}" fill="{fill}" {attrs}/>'


def rect(x, y, w, h, r, fill, attrs=""):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{fill}" {attrs}/>'


def ellipse(x, y, rx, ry, fill, attrs=""):
    return f'<ellipse cx="{x}" cy="{y}" rx="{rx}" ry="{ry}" fill="{fill}" {attrs}/>'


def circle(x, y, r, fill, attrs=""):
    return ellipse(x, y, r, r, fill, attrs)


def line(d, fill, width, attrs=""):
    return path(d, "none", f'stroke="{fill}" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round" {attrs}')


def group(body, attrs=""):
    return f'<g {attrs}>{body}</g>'


def linear(name, stops, coords=(0, 0, 0, 1024)):
    x1, y1, x2, y2 = coords
    content = ""
    for stop in stops:
        offset, colour, *alpha = stop
        content += f'<stop offset="{offset}" stop-color="{colour}" stop-opacity="{alpha[0] if alpha else 1}"/>'
    return f'<linearGradient id="{name}" gradientUnits="userSpaceOnUse" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}">{content}</linearGradient>'


def radial(name, stops, cx, cy, rx, ry=None):
    ry = ry or rx
    content = "".join(f'<stop offset="{o}" stop-color="{c}" stop-opacity="{a}"/>' for o, c, a in stops)
    return f'<radialGradient id="{name}" gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate({cx} {cy}) scale({rx} {ry})">{content}</radialGradient>'


def superellipse(inset=100):
    radius = 512 - inset
    points = []
    for i in range(768):
        a = i * math.tau / 768
        c, s = math.cos(a), math.sin(a)
        x = 512 + radius * math.copysign(abs(c) ** (2 / TILE_POWER), c)
        y = 512 + radius * math.copysign(abs(s) ** (2 / TILE_POWER), s)
        points.append(f'{x:.3f} {y:.3f}')
    return "M" + "L".join(points) + "Z"


def gear(cx=512, cy=512, radius=286, hole=119):
    points = []
    for i in range(10):
        for phase, r in [(-.48,radius*.81),(-.27,radius*.81),(-.21,radius),(.21,radius),(.27,radius*.81),(.48,radius*.81)]:
            angle = (i+phase)*math.tau/10-math.pi/2
            points.append((cx+math.cos(angle)*r,cy+math.sin(angle)*r))
    rounded = []
    for i,p in enumerate(points):
        before,after = points[(i-1)%len(points)],points[(i+1)%len(points)]
        def inset(q):
            distance = math.dist(p,q)
            amount = min(10,distance*.32)/distance
            return p[0]+(q[0]-p[0])*amount,p[1]+(q[1]-p[1])*amount
        a,b = inset(before),inset(after)
        rounded.append((p,a,b))
    start = rounded[-1][2]
    d = f'M{start[0]:.2f} {start[1]:.2f}'
    for p,a,b in rounded:
        d += f'L{a[0]:.2f} {a[1]:.2f}Q{p[0]:.2f} {p[1]:.2f} {b[0]:.2f} {b[1]:.2f}'
    return d+f'ZM{cx-hole} {cy}A{hole} {hole} 0 1 0 {cx+hole} {cy}A{hole} {hole} 0 1 0 {cx-hole} {cy}Z'


# Shared glass, paper and sky-blue folder colours.
PALETTE = {
    "files": [(0, "#50bdf0"), (.24, "#258ddd"), (.63, "#2169c8"), (1, "#254caa")],
    "settings": [(0, "#8495ba"), (.28, "#60749c"), (.65, "#39496f"), (1, "#252e4b")],
    "glass": [(0, "#f2fcff", .84), (.22, "#c6edff", .53), (.57, "#8acdf3", .28), (.82,"#82c9f0",.32), (1, "#b2e8ff", .74)],
    "back": [(0, "#b9e6fb"), (.3, "#8acaf1"), (.72, "#549ad5"), (1, "#3a79b5")],
    "front": [(0, "#c7edfc"), (.14, "#aedffa"), (.52, "#8dc9ef"), (.88, "#7ab9e5"), (1, "#6ba8d7")],
    "mark": "#4683b4", "paper": "#6488ab", "accent": "#b75962",
}


class Artwork:
    def __init__(self, size=1024):
        self.p = PALETTE
        self.size = size
        self.small = size <= 32
        self.defs = []
        self.defs += [linear("tile-files", self.p["files"], (400, 100, 640, 924))]
        self.defs += [linear("tile-settings", self.p["settings"], (350, 100, 650, 924))]
        self.defs += [linear("glass", self.p["glass"], (300, 300, 700, 830))]
        self.defs += [linear("folder-back", self.p["back"], (420, 150, 570, 860))]
        self.defs += [linear("folder-front", self.p["front"], (430, 350, 560, 870))]
        self.defs += [linear("rim", [(0,"#ffffff",.94),(.26,"#f4fcff",.43),(.57,"#c1def8",.07),(.85,"#c4e3fd",.12),(1,"#def4ff",.28)], (380, 180, 600, 850))]
        self.defs += [linear("glass-rim", [(0,"#fff",1),(.23,"#effaff",.78),(.53,"#c0ddff",.25),(.78,"#5d83b1",.52),(1,"#c9e6ff",.9)], (300, 280, 700, 810))]
        self.defs += [linear("glass-bottom", [(0,"#d4edff",.0),(.6,"#b4daff",.0),(.85,"#b0dcff",.17),(1,"#e1f4ff",.6)], (0, 450, 0, 810))]
        self.defs += [linear("inner-rim", [(0,"#274b76",.60),(.28,"#7dadd1",.24),(.56,"#c7e7ff",.20),(1,"#e8f9ff",.62)], (0,370,0,652))]
        self.defs += [radial("top-light", [(0,"#f1fcff",.43),(.43,"#d3eeff",.10),(1,"#d3eeff",0)], LIGHT_POSITION[0], LIGHT_POSITION[1], 720, 680)]
        self.defs += [radial("bottom-shade", [(0,"#12244b",.32),(1,"#12244b",0)], 820, 890, 720, 560)]
        self.defs += [radial("glass-glow", [(0,"#ffffff",.52),(.42,"#e7faff",.13),(1,"#e7faff",0)], 335, 366, 510, 500)]
        self.defs += [radial("edge-glow", [(0,"#ffffff",.85),(.28,"#effaff",.28),(.65,"#d6ebff",.03),(1,"#d6ebff",.0)], 295, 325, 360, 260)]
        self.defs += [radial("glass-caustic", [(0,"#d2ebff",.50),(.24,"#a3d3ff",.14),(1,"#7ebeff",.0)], 544, 753, 246, 44)]
        self.defs += [radial("gear-refraction", [(0,"#9edcf6",.50),(.44,"#a9d6f5",.10),(1,"#94c9f3",0)], 345, 673, 223, 163)]
        self.defs += ['<radialGradient id="ground" cx=".5" cy=".5" r=".5"><stop stop-color="#14233b" stop-opacity=".24"/><stop offset=".38" stop-color="#14233b" stop-opacity=".11"/><stop offset="1" stop-color="#14233b" stop-opacity="0"/></radialGradient>']
        self.defs += [linear("paper", [(0,"#ffffff"),(.35,"#ffffff"),(.72,"#f8fafc"),(1,"#e9eef3")], (300, 120, 650, 914))]
        self.defs += [linear("paper-fold", [(0,"#edf2f6"),(.38,"#e2e8ef"),(.8,"#f5f7fa"),(1,"#ffffff")], (650, 125, 790, 275))]
        self.defs += [linear("paper-edge", [(0,"#bbc8d5",.75),(.25,"#a4b2c2",.25),(.75,"#9baabc",.55),(1,"#7c91a7",.75)], (240, 160, 770, 910))]
        self.defs += [linear("bin", [(0,"#838f9a",.88),(.1,"#b5bfc7",.81),(.24,"#e6ebed",.87),(.43,"#d2dade",.77),(.66,"#b1bdc5",.82),(.84,"#8d9ba7",.91),(1,"#6e7d8b",.92)], (175, 0, 845, 0))]
        self.defs += [linear("bin-shade", [(0,"#f5f8fa",.42),(.22,"#f5f8fa",0),(.72,"#334452",.03),(1,"#344757",.26)], (0, 260, 0, 888))]
        self.defs += [linear("metal", [(0,"#c9d3dc"),(.10,"#f9fcff"),(.25,"#a8b6c3"),(.51,"#d4dfe6"),(.79,"#edf3f7"),(1,"#738699")], (240, 220, 730, 830))]
        self.defs += [linear("lens", [(0,"#effaff",.93),(.30,"#d1ebff",.84),(.57,"#a2c4eb",.90),(1,"#81a9d9",.96)], (350, 300, 670, 750))]
        self.defs += [radial("lens-reflection", [(0,"#ffffff",.82),(.35,"#edfaff",.23),(.75,"#d7edff",.03),(1,"#d7edff",.15)], 425, 352, 255, 285)]
        self.defs += [linear("slot", [(0,"#18304d",.8),(.2,"#294664",.67),(.55,"#284c73",.54),(1,"#92b3d4",.42)], (320, 300, 680, 760))]
        self.defs += [linear("impression", [(0,self.p["mark"],.98),(.35,self.p["mark"],.87),(1,self.p["mark"],.70)], (0,450,0,740))]
        self.defs += [linear("folder-lip", [(0,"#f8fdff",.9),(.20,"#e5f5ff",.8),(.65,"#bce0f8",.25),(1,"#6b9dc6",.10)], (0,351,0,375))]
        self.defs += [radial("folder-volume", [(0,"#1f568a",.0),(.86,"#1f568a",.0),(1,"#1f568a",.20)], 510,570,470,420)]
        self.defs += [linear("bin-inside", [(0,"#6c7c8b"),(.32,"#8b9aa7"),(.7,"#c4cfd7"),(1,"#e7edf1")], (400, 210, 560, 400))]
        for name, blur in [("wide", 18),("contact", 8),("soft", 4),("fine", 1.5)]:
            self.defs += [f'<filter id="{name}" x="-50%" y="-60%" width="200%" height="240%" color-interpolation-filters="sRGB"><feGaussianBlur stdDeviation="{blur}"/></filter>']
        self.defs += ['''<filter id="glass-bevel" x="-.2" y="-.2" width="1.4" height="1.4" color-interpolation-filters="sRGB">
          <feGaussianBlur in="SourceAlpha" stdDeviation="6" result="alpha-soft"/>
          <feOffset in="alpha-soft" dx="-3" dy="-6" result="shadow-offset"/>
          <feComposite in="SourceAlpha" in2="shadow-offset" operator="out" result="shadow-edge"/>
          <feFlood flood-color="#214b78" flood-opacity=".48" result="shadow-colour"/>
          <feComposite in="shadow-colour" in2="shadow-edge" operator="in" result="inner-shadow"/>
          <feOffset in="alpha-soft" dx="2" dy="5" result="light-offset"/>
          <feComposite in="SourceAlpha" in2="light-offset" operator="out" result="light-edge"/>
          <feFlood flood-color="#ffffff" flood-opacity=".9" result="light-colour"/>
          <feComposite in="light-colour" in2="light-edge" operator="in" result="inner-light"/>
          <feMerge><feMergeNode in="SourceGraphic"/><feMergeNode in="inner-shadow"/><feMergeNode in="inner-light"/></feMerge>
        </filter>''']

    def clip(self, name, body):
        self.defs.append(f'<clipPath id="{name}">{body}</clipPath>')

    def shadow(self, body, x=0, y=15, strength=.25, blur="contact"):
        return group(body, f'opacity="{strength}" transform="translate({x} {y})" filter="url(#{blur})"')

    def svg(self, body):
        return f'<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024"><defs>{"".join(self.defs)}</defs>{body}</svg>'

    def tile(self, name):
        d = superellipse((CANVAS - TILE_SIZE)/2)
        self.clip("tile-clip", path(d,"#fff"))
        result = self.shadow(path(d,"#111f38"), y=19, strength=.27, blur="wide")
        result += path(d,f'url(#tile-{name})')
        result += path(d,"url(#top-light)")
        result += path(d,"url(#bottom-shade)")
        # The dark lower inset and bright upper rim give the tile a softly rolled edge.
        result += group(path(d,"none",'stroke="#0a1b37" stroke-opacity=".28" stroke-width="9" transform="translate(0 5)"'), 'clip-path="url(#tile-clip)"')
        result += path(superellipse(102),"none",'stroke="url(#rim)" stroke-width="3"')
        return result

    def files(self):
        result = self.tile("files")
        tile = result
        result = ''
        # A quiet back leaf is seen through the front glass, rather than floating as a badge.
        back = 'M258 384V326C258 300 276 286 302 286H420C439 286 448 296 465 314L493 342H734C758 342 774 359 774 385V704C774 735 755 752 726 752H302C274 752 258 735 258 706Z'
        face = 'M249 430C247 411 263 398 284 398H749C772 398 784 414 780 435L738 725C735 752 720 768 694 768H334C308 768 293 753 289 727Z'
        result += self.shadow(path(back,"#142f5d"),y=23,strength=.35,blur="wide")
        result += path(back,"url(#glass)", 'stroke="url(#glass-rim)" stroke-width="2" filter="url(#glass-bevel)"')
        leaf = rect(306, 365, 408, 326, 24, "#e5f3ff", 'fill-opacity=".67" transform="rotate(-6 510 600)"')
        result += self.shadow(rect(306, 370, 408, 326, 24, "#284f8b",'transform="rotate(-6 510 600)"'),y=7,strength=.2)
        result += leaf
        result += line('M337 390L672 355', '#ffffff', 3, 'opacity=".73"')
        result += self.shadow(path(face,"#102f6d"),y=18,strength=.35,blur="contact")
        result += path(face,"url(#glass)", 'stroke="url(#glass-rim)" stroke-width="2" filter="url(#glass-bevel)"')
        self.clip("front-clip",path(face,"#fff"))
        result += group(path(face,"url(#glass-glow)")+path(face,"url(#glass-bottom)")+path(face,"url(#edge-glow)")+path(face,"url(#glass-caustic)"), 'clip-path="url(#front-clip)"')
        edge = 'M284 403H744'
        result += line(edge, '#fff', 2, 'opacity=".68"')
        # Broad refraction in the overlap is clipped to the glass silhouette.
        result += group(rect(307,421,404,290,22,"#e4f5ff",'opacity=".10" transform="rotate(-6 510 600)" filter="url(#soft)"'), 'clip-path="url(#front-clip)"')
        return tile+group(result,'transform="translate(-128 -146) scale(1.25)"')

    def settings(self):
        result = self.tile("settings")
        # A single deep glass ring has broad teeth and a clear central opening.
        d = gear(radius=309,hole=133)
        result += self.shadow(path(d,"#101d39",'fill-rule="evenodd"'),y=25,strength=.46,blur="wide")
        result += path(d,"#9ccef3",'fill-rule="evenodd" transform="translate(0 8)" opacity=".24"')
        result += path(d,"url(#glass)",'fill-rule="evenodd" stroke="url(#glass-rim)" stroke-width="2.5" filter="url(#glass-bevel)"')
        self.clip("gear-clip",path(d,"#fff",'fill-rule="evenodd"'))
        result += group(path(d,"url(#lens-reflection)",'fill-rule="evenodd"')+path(d,"url(#glass-caustic)",'fill-rule="evenodd"')+path(d,"url(#gear-refraction)",'fill-rule="evenodd"'), 'clip-path="url(#gear-clip)"')
        result += circle(512,512,137,"none",'stroke="url(#inner-rim)" stroke-width="9"')
        result += circle(512,509,132,"none",'stroke="url(#rim)" stroke-width="1.5"')
        return result

    def folder(self, mark=None, opened=False):
        # The broad, nearly full-width face balances the tall document optically.
        back = 'M89 386V257C89 227 108 209 139 209H341C364 209 377 217 390 232L437 285H872C908 285 929 307 929 341V790C929 825 908 847 874 847H143C108 847 89 827 89 792Z'
        front = 'M76 398C73 371 93 351 121 351H902C932 351 950 373 946 401L889 805C885 839 862 857 831 857H192C158 857 138 838 133 805Z'
        if opened:
            front = 'M63 464C57 436 77 415 108 415H916C947 415 967 438 960 467L882 805C875 839 854 857 823 857H198C164 857 143 839 136 805Z'
        result = ellipse(512,891,385,60,"url(#ground)")
        result += self.shadow(path(back,"#1d497b"),y=14,strength=.18,blur="contact")
        result += path(back,"url(#folder-back)", 'stroke="#6a9cc5" stroke-opacity=".7" stroke-width="2"')
        result += line('M100 308V257C100 232 115 220 140 220H338C357 220 367 225 381 240L431 296H869C895 296 911 312 913 335', '#edfaff', 3, 'opacity=".78"')
        # A narrow recess separates the back panel and the front flap.
        result += rect(110,325,800,92,22,"#447fae",'opacity=".43"')
        result += rect(115,332,794,80,18,"#cfeeff",'opacity=".55"')
        result += self.shadow(path(front,"#1d4878"),y=12,strength=.27,blur="soft")
        result += path(front,"url(#folder-front)",'stroke="#6193c0" stroke-opacity=".65" stroke-width="2"')
        self.clip("folder-clip",path(front,"#fff"))
        result += group(path(front,"url(#glass-glow)", 'opacity=".28"') + path(front,"url(#folder-volume)") + rect(63,415 if opened else 351,897,23,10,"url(#folder-lip)") + ellipse(530,842,430,54,"#487dab",'opacity=".12" filter="url(#wide)"'), 'clip-path="url(#folder-clip)"')
        top_edge = 'M119 355H898C922 355 939 374 936 397' if not opened else 'M108 419H916C942 419 958 439 953 462'
        result += line(top_edge, '#f5fcff', 3, 'opacity=".95"')
        result += line('M88 399L145 802C150 832 168 846 195 846H827C857 846 875 832 880 803', '#d6f0ff', 2.5, 'opacity=".53"')
        result += line('M933 410L878 798', '#4b83b5', 4, 'opacity=".22"')
        if mark:
            # A solid, deep impression has a lower lip and a shaded upper bevel.
            mark = folder_mark(mark)
            glyph = path(mark,"#e1f5ff",'fill-rule="evenodd" transform="translate(0 5)" opacity=".78"')
            glyph += path(mark,"url(#impression)", 'fill-rule="evenodd"')
            self.clip("mark-clip",path(mark,"#fff", 'fill-rule="evenodd"'))
            glyph += group(path(mark,"none",'stroke="#285880" stroke-width="6" transform="translate(0 4)" opacity=".3" filter="url(#fine)"'), 'clip-path="url(#mark-clip)"')
            transform = 'translate(61 74) scale(.88)'
            if self.size <= 32:
                transform = 'translate(-61.44 -72) scale(1.12)'
            if self.size <= 16:
                transform = 'translate(-102.4 -120) scale(1.2)'
            result += group(glyph,f'transform="{transform}"')
        return result

    def document(self, kind="generic"):
        page = 'M240 112H640L800 274V867C800 894 782 912 755 912H240C213 912 195 894 195 867V157C195 130 213 112 240 112Z'
        result = ellipse(500,930,320,40,"url(#ground)")
        result += self.shadow(path(page,"#25364b"),y=10,strength=.19,blur="contact")
        result += path(page,"url(#paper)",'stroke="url(#paper-edge)" stroke-width="2"')
        result += line('M242 115H637M198 160V862C198 889 214 906 241 906', '#ffffff', 3, 'opacity=".9"')
        fold = 'M640 112V239C640 261 653 274 676 274H800Z'
        result += self.shadow(path(fold,"#667587"),x=-2,y=6,strength=.24,blur="soft")
        result += path(fold,"url(#paper-fold)")
        result += line('M642 119V238C642 259 654 271 676 271H791','#ffffff',2.5,'opacity=".94"')
        result += self.document_content(kind)
        return result

    def trash(self, full):
        body = 'M188 315L251 823C256 872 364 897 512 897C660 897 768 872 773 823L836 315Z'
        result = ellipse(512,922,350,52,"url(#ground)")
        result += self.shadow(path(body,"#2f3d4d"),y=10,strength=.18,blur="contact")
        result += path(body,"url(#bin)")
        result += ellipse(512,315,324,90,"url(#bin-inside)")
        result += ellipse(512,315,324,90,"none",'stroke="#657788" stroke-opacity=".65" stroke-width="12"')
        result += ellipse(512,311,324,90,"none",'stroke="url(#metal)" stroke-width="10"')
        if full:
            # Folded white scraps sit below and above the open rim.
            paper1 = 'M282 373L270 206L329 145L493 176L534 359L444 445Z'
            paper2 = 'M464 371L517 139L644 117L730 188L728 393L622 445Z'
            result += path(paper1,"url(#paper)",'stroke="#b0bac5" stroke-width="2"')
            result += path(paper2,"url(#paper)",'stroke="#a5b0be" stroke-width="2"')
            result += path('M517 139L568 216L644 117L730 188L645 242L568 216L527 348Z',"#e1e8ee",'opacity=".87"')
            result += path('M270 206L358 254L329 145L493 176L427 274L358 254L361 393Z',"#eef2f5")
            result += line('M287 214L358 254L427 274L493 176M358 254L361 393M518 142L568 216L645 242L730 188M645 242L621 394', '#acbac7',2,'opacity=".52"')
            result += line('M391 325L451 337M393 343L457 354M579 270L668 292M575 291L663 312', '#92a0b0',6,'opacity=".37"')
        # A translucent front wall lets the pale paper show through the neutral ribs.
        front = 'M188 315C188 364 333 405 512 405C691 405 836 364 836 315L773 823C768 872 660 897 512 897C364 897 256 872 251 823Z'
        result += path(front,"url(#bin)",'opacity=".84"')
        self.clip("bin-front",path(front,"#fff"))
        if full:
            inside = path('M330 420L372 647L456 714L512 588L474 435Z', '#f7f9fa','opacity=".28" filter="url(#soft)"')
            inside += path('M571 429L540 622L623 716L688 560L672 426Z', '#f7f9fa','opacity=".23" filter="url(#soft)"')
            result += group(inside, 'clip-path="url(#bin-front)"')
        result += path(front,"url(#bin-shade)")
        # The rib spacing grows optically in the small edition so each flute survives reduction.
        count = 25 if not self.small else 17
        ribs = ''
        for i in range(1,count):
            a = -math.pi/2 + math.pi*i/count
            x = 512+316*math.sin(a)
            bottom_x = 512+250*math.sin(a)
            y = 325+78*math.cos(a)
            bottom_y = 815+76*math.cos(a)
            ribs += line(f'M{x:.1f} {y+12:.1f}L{bottom_x:.1f} {bottom_y-7:.1f}', '#f7fcff', 4 if self.small else 3, f'opacity="{.32+.25*math.cos(a):.2f}"')
            ribs += line(f'M{x+5:.1f} {y+14:.1f}L{bottom_x+4:.1f} {bottom_y-9:.1f}', '#465969', 4 if self.small else 3, 'opacity=".16"')
        result += group(ribs,'clip-path="url(#bin-front)"')
        result += line('M199 326L261 821C266 859 368 886 511 886C654 886 758 861 764 822L825 327','#e6eef4',3,'opacity=".61"')
        result += path('M188 315C188 364 333 405 512 405C691 405 836 364 836 315',"none",'stroke="#657788" stroke-opacity=".65" stroke-width="12"')
        result += path('M188 311C188 360 333 401 512 401C691 401 836 360 836 311',"none",'stroke="url(#metal)" stroke-width="10"')
        result += path('M192 311C192 360 334 400 512 400C690 400 831 361 831 311',"none",'stroke="#f1f6f9" stroke-width="4" opacity=".83"')
        return result if full else group(result, 'transform="translate(-51.2 -89.7) scale(1.1)"')

    def generic(self):
        self.defs += [linear('tile-generic', [(0,'#9caed0'),(.28,'#7186b1'),(.66,'#4b608e'),(1,'#334269')], (350,100,630,924))]
        result = self.tile('generic')
        window = rect(214,247,596,551,64,'#112d56')
        result += self.shadow(window, y=23, strength=.4, blur='wide')
        result += rect(214,247,596,551,64,'url(#glass)', 'stroke="url(#glass-rim)" stroke-width="3" filter="url(#glass-bevel)"')
        self.clip('window-clip',rect(214,247,596,551,64,'#fff'))
        result += group(rect(214,247,596,128,0,'#e4f5ff','opacity=".30"')+rect(214,375,144,423,0,'#243d6c','opacity=".23"'), 'clip-path="url(#window-clip)"')
        result += line('M218 375H806','#f3fbff',3,'opacity=".55"')
        result += line('M358 380V792','#c4e4ff',2,'opacity=".33"')
        result += rect(398,423,359,318,31,'url(#glass)','opacity=".46"')
        result += rect(398,423,359,318,31,'none','stroke="url(#rim)" stroke-width="2"')
        if not self.small:
            for x in (267,307,347):
                result += circle(x,309,9,'#f3fbff','opacity=".66"')
        else:
            result += circle(277,309,14,'#f3fbff','opacity=".66"')
        result += group(rect(214,247,596,551,64,'url(#glass-glow)'),'clip-path="url(#window-clip)"')
        return result

    def document_content(self, kind):
        ink = {'text':'#6488ab','generic':'#8c99aa','pdf':'#b75962','image':'#7a8fac','video':'#747aa0','audio':'#5d9694','archive':'#ac926e','executable':'#5a8b88'}[kind]
        result = ''
        if kind in ('text','generic'):
            result += rect(266,328,316 if kind == 'text' else 240,26,5,ink,'opacity=".85"')
            rows = [(393,446),(433,408),(473,440),(552,444),(592,387),(632,423),(711,441),(751,298)]
            if kind == 'generic':
                rows = [(420,440),(474,380),(528,419),(628,295)]
            if self.size <= 16:
                rows = [(420,446),(510,408),(600,440),(690,340)]
            elif self.size <= 32:
                rows = [(410,446),(470,408),(530,440),(620,444),(680,387),(740,340)]
            for y,w in rows:
                result += rect(266,y,w,13 if not self.small else 27,4,'#99a7b7','opacity=".59"')
            if not self.small:
                result += rect(266,818,50,6,3,ink,'opacity=".7"')
            return result
        if kind == 'pdf':
            result += rect(266,335,13 if not self.small else 25,427,6.5,ink,'opacity=".83"')
            result += rect(321,335,282,23,5,ink,'opacity=".84"')
            if not self.small:
                for y,w in ((389,366),(424,322),(690,349),(725,282)):
                    result += rect(321,y,w,12,4,'#a2aebc','opacity=".65"')
                result += rect(321,488,372,166,9,'#eef1f5')
                result += line('M345 626L414 575L469 592L563 527L668 556',ink,9,'opacity=".57"')
                # Letter outlines are part of the drawing and never require a font.
                label = 'M524 824V777H539Q556 777 556 790Q556 803 539 803H524M574 824V777H585Q609 777 609 800Q609 824 585 824ZM628 824V777H659M628 799H651'
                result += group(line(label,ink,7),'transform="translate(-198 -287) scale(1.35)"')
            else:
                result += rect(321,420,360,25,6,'#a2aebc','opacity=".6"')
                result += rect(321,510,372,143,9,'#eef1f5')
                result += line('M345 624L448 567L512 589L654 534',ink,18,'opacity=".65"')
                result += rect(321,714,282,25,6,'#a2aebc','opacity=".6"')
            return result
        result += rect(266,335,176,22 if not self.small else 30,5,ink,'opacity=".83"')
        if kind in ('image','video'):
            self.defs += [linear('preview',[(0,'#dce9f3'),(.5,'#b3cde2'),(1,'#7b9bbd')],(300,385,650,700))]
            preview = rect(266,393,447,307,12,'url(#preview)')
            result += preview
            self.clip('preview-clip',rect(266,393,447,307,12,'#fff'))
            if kind == 'image':
                picture = circle(601,466,34,'#f5e8c9')
                picture += path('M252 706L403 476Q412 462 422 478L577 706Z','#7c9bb7')
                picture += path('M429 706L568 533Q577 522 586 534L733 706Z','#547598')
                picture += path('M375 519L403 476Q412 462 422 478L456 529L423 517L409 529L399 516Z','#edf4f9','opacity=".86"')
                result += group(picture,'clip-path="url(#preview-clip)"')
            else:
                result += rect(266,393,447,307,12,'#3f507c','opacity=".63"')
                result += path('M449 456Q437 449 437 463V631Q437 646 450 638L583 558Q595 550 583 542Z','#eef5fc','opacity=".95"')
            result += rect(266,393,447,307,12,'none','stroke="#718ba4" stroke-opacity=".35" stroke-width="2"')
        elif kind == 'audio':
            for i,h in enumerate((92,156,236,292,215,132,76)):
                x = 292+i*60
                result += rect(x,550-h/2,25 if not self.small else 32,h,12,ink,'opacity=".8"')
        elif kind == 'archive':
            result += rect(466,393,66,349,10,'#ded6c9')
            result += rect(491,393,13,349,3,'#a08c72','opacity=".46"')
            for i in range(6 if self.small else 10):
                y = 411+i*(39 if self.small else 24)
                result += rect(475 if i%2 else 500,y,24,17,4,ink)
            result += rect(466,637,67,77,15,'url(#metal)','stroke="#a19582" stroke-width="3"')
            result += rect(485,656,29,33,8,'#968573')
            result += line('M279 430H395M279 480H374M589 430H700M589 480H671','#a2aebc',14 if not self.small else 25,'opacity=".6"')
        else:
            result += line('M378 442L287 542L378 642M602 442L693 542L602 642M534 419L452 660',ink,26 if not self.small else 39)
        result += rect(266,764,447,13 if not self.small else 25,4,'#a2aebc','opacity=".6"')
        if not self.small:
            result += rect(266,805,299,13,4,'#a2aebc','opacity=".6"')
        return result

    def computer(self):
        self.defs += [linear('display-metal',[(0,'#e3eaf0'),(.18,'#c9d5df'),(.52,'#a4b5c5'),(.82,'#c7d3df'),(1,'#7c8fa5')],(280,180,740,810))]
        self.defs += [linear('display-sky',[(0,'#a6cee8'),(.25,'#709fca'),(.65,'#4e76b4'),(1,'#38497c')],(190,200,730,690))]
        self.defs += [linear('display-wave',[(0,'#99cde4'),(.48,'#8daed9'),(1,'#656fac')],(300,420,750,700))]
        result = ellipse(512,928,344,45,'url(#ground)')
        stand = 'M461 718H563L594 865H430Z'
        result += self.shadow(path(stand,'#233c58'),y=12,strength=.22)
        result += path(stand,'url(#display-metal)','stroke="url(#rim)" stroke-width="3"')
        body = rect(72,160,880,591,49,'#20364e')
        result += self.shadow(body,y=15,strength=.24,blur='wide')
        result += rect(72,160,880,591,49,'url(#display-metal)','stroke="url(#rim)" stroke-width="3"')
        result += rect(94,181,836,501,29,'#243c58')
        result += rect(103,190,818,483,22,'url(#display-sky)')
        self.clip('display-clip',rect(103,190,818,483,22,'#fff'))
        waves = path('M72 620C270 324 441 314 634 474C757 576 852 494 952 364V725H72Z','url(#display-wave)')
        waves += path('M72 733C268 487 450 450 631 553C784 639 862 598 952 518V725H72Z','#c4dff0','opacity=".65"')
        waves += path('M114 214C299 148 529 189 656 322C415 231 240 311 114 416Z','#e4f4ff','opacity=".13"')
        result += group(waves,'clip-path="url(#display-clip)"')
        result += line('M120 177H899','#f5fbff',3,'opacity=".75"')
        result += line('M103 675H920','#f3faff',2,'opacity=".53"')
        base = 'M429 854H595C624 854 642 870 675 892Q686 900 680 911H345Q339 900 350 892C383 870 400 854 429 854Z'
        result += self.shadow(path(base,'#233c58'),y=8,strength=.22,blur='soft')
        result += path(base,'url(#display-metal)','stroke="url(#rim)" stroke-width="3"')
        result += line('M362 901H665','#eff8ff',3,'opacity=".83"')
        if not self.small:
            result += circle(512,715,5,'#eff8ff','opacity=".6"')
        return result

    def badge(self, locked):
        if locked:
            self.defs += [linear('brass',[(0,'#fae8c6'),(.23,'#edd2a1'),(.6,'#d6ae72'),(.85,'#c99c5f'),(1,'#e2bd87')],(300,400,700,928))]
            arch = 'M292 472V321C292 184 376 111 512 111C648 111 732 184 732 321V472H650V324C650 237 601 192 512 192C423 192 374 237 374 324V472Z'
            result = self.shadow(path(arch,'#374656'),y=12,strength=.25)
            result += path(arch,'url(#metal)','stroke="url(#glass-rim)" stroke-width="4"')
            result += self.shadow(rect(172,431,680,488,83,'#6d4b2d'),y=12,strength=.25)
            result += rect(172,431,680,488,83,'url(#brass)','stroke="#f5dfb8" stroke-width="3" filter="url(#glass-bevel)"')
            result += rect(172,431,680,488,83,'url(#glass-glow)','opacity=".45"')
            r = 42 if not self.small else 54
            result += circle(512,625,r,'#826342')
            result += rect(493 if not self.small else 485,635,38 if not self.small else 54,111,19,'#826342')
            result += line('M259 439H767','#fff6e7',3,'opacity=".88"')
            return result
        arrow = 'M174 826V609C174 435 284 344 467 344H571V209C571 190 583 184 598 197L883 435C901 450 901 468 883 483L598 720C583 733 571 726 571 708V571H467C381 571 328 609 328 693V826C328 846 314 860 294 860H208C188 860 174 846 174 826Z'
        result = self.shadow(path(arrow,'#253e5f'),y=12,strength=.26)
        result += path(arrow,'url(#paper)','stroke="#3f6386" stroke-width="38" stroke-linejoin="round"' if self.small else 'stroke="#7396b7" stroke-width="9" stroke-linejoin="round"')
        result += path(arrow,'url(#glass)','opacity=".22" filter="url(#glass-bevel)"')
        return group(result,'transform="translate(-77 -80) scale(1.15)"')


def folder_mark(kind):
    if kind == 'downloads':
        return 'M487 463Q487 453 497 453H527Q537 453 537 463V582L584 535Q591 528 598 535L616 553Q623 560 616 568L524 661Q512 673 500 661L408 568Q401 560 408 553L426 535Q433 528 440 535L487 582ZM388 651Q388 642 398 642H421Q430 642 430 651V699H594V651Q594 642 604 642H626Q636 642 636 651V720Q636 737 619 737H405Q388 737 388 720Z'
    # Solid cutouts use the same 256-unit drawing grid before being placed on the front flap.
    if kind == 'home':
        d = 'M16 117L111 28Q128 12 145 28L240 117Q249 126 240 135L228 148L208 130V227H145V165H111V227H48V130L28 148L16 135Q7 126 16 117Z'
    elif kind == 'applications':
        return ''.join(f'M{x+9+384} {y+448}H{x+69+384}Q{x+78+384} {y+448} {x+78+384} {y+9+448}V{y+69+448}Q{x+78+384} {y+78+448} {x+69+384} {y+78+448}H{x+9+384}Q{x+384} {y+78+448} {x+384} {y+69+448}V{y+9+448}Q{x+384} {y+448} {x+9+384} {y+448}Z' for x in (32,146) for y in (32,146))
    elif kind == 'desktop':
        d = 'M29 30H227Q241 30 241 44V171Q241 185 227 185H143V208H184V232H72V208H113V185H29Q15 185 15 171V44Q15 30 29 30ZM43 58V157H213V58Z'
    elif kind == 'documents':
        d = 'M63 16H155L205 68V227Q205 240 192 240H63Q50 240 50 227V29Q50 16 63 16ZM78 43V212H177V87H148Q135 87 135 74V43ZM93 111H162V131H93ZM93 151H162V171H93Z'
    elif kind == 'music':
        d = 'M107 57L223 30V183C223 215 197 231 172 222C151 214 154 186 174 176L195 170V79L135 93V204C135 234 109 248 85 238C64 229 68 202 88 193L107 187Z'
    elif kind == 'movies':
        d = 'M28 27H228Q241 27 241 40V218Q241 231 228 231H28Q15 231 15 218V40Q15 27 28 27ZM74 56V202H182V56ZM35 54V78H53V54ZM35 115V141H53V115ZM35 179V204H53V179ZM203 54V78H221V54ZM203 115V141H221V115ZM203 179V204H221V179Z'
    else:
        d = 'M29 27H227Q241 27 241 41V216Q241 230 227 230H29Q15 230 15 216V41Q15 27 29 27ZM43 55V185L99 119Q106 111 113 119L151 161L176 136Q184 128 192 137L213 160V55ZM63 88A21 21 0 1 0 105 88A21 21 0 1 0 63 88Z'
    # Translate the hand-drawn contours without relying on a glyph font.
    import re
    tokens = re.findall(r'[A-Za-z]|-?\d+(?:\.\d+)?',d)
    result, command, index = '', '', 0
    for token in tokens:
        if token.isalpha():
            command, index = token, 0
            result += token
        else:
            n = float(token)
            if command == 'H':
                n += 384
            elif command == 'V':
                n += 448
            elif command == 'A':
                if index%7 == 5:
                    n += 384
                elif index%7 == 6:
                    n += 448
            else:
                n += 384 if index%2 == 0 else 448
            result += f'{n:g} '
            index += 1
    return result


def render(name, small, symbols=None):
    art = Artwork(small or 1024)
    if name == 'application.file-manager':
        body = art.files()
    elif name == 'application.settings':
        body = art.settings()
    elif name == 'application.generic':
        body = art.generic()
    elif name.startswith('folder'):
        mark = name.split('.')[-1] if '.' in name else None
        body = art.folder(None if mark == 'generic-open' else mark, mark == 'generic-open')
    elif name.startswith('document.'):
        body = art.document(name.split('.')[-1])
    elif name.startswith('trash.'):
        body = art.trash(name.endswith('full'))
    elif name == 'computer':
        body = art.computer()
    else:
        body = art.badge(name == 'locked.badge')
    return art.svg(body)+'\n'
