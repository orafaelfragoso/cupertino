"""Original layered icon constructions on a common superellipse keyline."""
import math

# The tile occupies 236 of 256 units, leaving room for its soft lower shadow.
TILE_INSET = 10
# A fourth-power superellipse keeps the same continuous corner on every tile.
TILE_POWER = 4


def squircle(inset=TILE_INSET):
    radius = 128 - inset
    points = []
    for i in range(1024):
        a = i * math.tau / 1024
        c, s = math.cos(a), math.sin(a)
        x = 128 + radius * math.copysign(abs(c) ** (2 / TILE_POWER), c)
        y = 128 + radius * math.copysign(abs(s) ** (2 / TILE_POWER), s)
        points.append(f'{x:.3f},{y:.3f}')
    return 'M' + 'L'.join(points) + 'Z'


def shape(d, fill, extra=''):
    return f'<path d="{d}" fill="{fill}" {extra}/>'


def box(x, y, w, h, r, fill, extra=''):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{fill}" {extra}/>'


def orb(x, y, r, fill, extra=''):
    return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{fill}" {extra}/>'


def line(d, color, width=2, extra=''):
    return f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round" {extra}/>'


def gradient(name, colors, x1=0, y1=0, x2=0, y2=256):
    stops = ''.join(f'<stop offset="{i/(len(colors)-1):.3f}" stop-color="{c}"/>' for i,c in enumerate(colors))
    return f'<linearGradient id="{name}" gradientUnits="userSpaceOnUse" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}">{stops}</linearGradient>'


def definitions(palette):
    return '<defs>' + gradient('tile', palette) + gradient('glass', ['#c5ffff','#49dafa','#0871da'], y1=70,y2=215) + gradient('ice',['#ffffff','#d4f9ff','#74c9ed'],y1=50,y2=205) + gradient('paper',['#ffffff','#ffffff','#d8e6f1'],y1=40,y2=216) + gradient('edge',['#fff','#99bad0','#edfaff'],y1=50,y2=210) + gradient('dark',['#626f80','#202b40','#101827'],y1=45,y2=200) + gradient('violet',['#ffdbff','#ad9cff','#6655e2'],y1=55,y2=205) + gradient('gold',['#fff5c3','#ffc663','#de7a13'],y1=60,y2=210) + gradient('mint',['#cbfff0','#44e1bb','#07998c'],y1=70,y2=215) + '<radialGradient id="light" cx=".3" cy="0" r="1"><stop stop-color="#fff" stop-opacity=".4"/><stop offset=".7" stop-color="#fff" stop-opacity="0"/></radialGradient><linearGradient id="rim" x2=".1" y2="1"><stop stop-color="#fff" stop-opacity=".8"/><stop offset=".5" stop-color="#fff" stop-opacity=".08"/><stop offset="1" stop-color="#fff" stop-opacity=".32"/></linearGradient><linearGradient id="sheen" x2=".7" y2="1"><stop stop-color="#fff" stop-opacity=".65"/><stop offset=".5" stop-color="#fff" stop-opacity=".1"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></linearGradient><filter id="shadow" x="-40%" y="-40%" width="180%" height="200%"><feGaussianBlur stdDeviation="4"/></filter><filter id="layer" x="-40%" y="-40%" width="180%" height="200%"><feGaussianBlur stdDeviation="2.2"/></filter><clipPath id="clip">'+shape(squircle(),'white')+'</clipPath></defs>'


def tile(small):
    d = squircle()
    result = shape(d,'#000','opacity=".35" transform="translate(0 5)" filter="url(#shadow)"')
    result += shape(d,'url(#tile)') + shape(d,'url(#light)')
    if not small:
        result += '<g clip-path="url(#clip)">'+shape(d,'none','stroke="#071125" stroke-opacity=".25" stroke-width="3" transform="translate(0 2)"')+'</g>'
    result += shape(squircle(10.7),'none',f'stroke="url(#rim)" stroke-width="{1.6 if small else 1.1}"')
    return result


def shadow(body, small=False):
    return '<g fill="#001b46" opacity=".25" transform="translate(0 4)" filter="url(#layer)">'+body+'</g>' if not small else ''


def folder(mark, small, symbols, opened=False):
    back = 'M47 103V83Q47 72 59 72H104Q111 72 116 79L129 92H198Q210 92 210 105V190Q210 202 198 202H58Q47 202 47 190Z'
    front = 'M43 113Q42 104 53 104H207Q217 104 215 116L202 192Q200 205 188 205H65Q53 205 51 192Z'
    if opened:
        front = 'M33 127Q30 116 43 116H214Q225 116 221 129L201 196Q198 205 186 205H63Q51 205 48 194Z'
    result = shadow(shape(back,'#001a53'),small)
    result += shape(back,'url(#ice)','stroke="#cfffff" stroke-width="1.3"')
    if not small:
        result += box(60,90,133,96,8,'#027acb','opacity=".22"')
        result += box(61,98,136,85,5,'url(#paper)','transform="rotate(-4 128 130)"')
        result += line('M60 104H196','#fff',1.5,'opacity=".7"')
    result += shape(front,'url(#glass)','stroke="#8eeaff" stroke-width="1.2"')
    result += shape('M55 112H203L198 137Q127 122 58 147Z','url(#sheen)','opacity=".6"')
    if not small:
        result += line('M62 115H198','#efffff',1.8,'opacity=".8"')
        result += line('M61 191Q62 198 70 198H187','#004fb2',2,'opacity=".22"')
        result += shape('M53 119L59 187Q61 199 70 199L65 129Z','#c5ffff','opacity=".3"')
        result += shape('M207 117L196 191Q195 198 187 200L196 131Z','#0061c9','opacity=".2"')
    if mark:
        # An offset dark impression and a lighter rim make the glyph feel pressed into glass.
        motif = symbols[mark]
        if small == 16:
            motif = {
                'document': line('M6 3H16V21H6ZM9 9H13M9 14H13','currentColor',2.5),
                'desktop': line('M3 5H21V17H3ZM12 17V21M8 21H16','currentColor',2.5),
                'film': line('M3 4H21V20H3ZM8 4V20M16 4V20','currentColor',2.5),
                'music': line('M10 18V5L19 3V15','currentColor',3)+orb(7,18,3,'currentColor')+orb(16,15,3,'currentColor'),
                'photo': line('M3 4H21V20H3ZM4 18L11 10L19 19','currentColor',2.5),
            }.get(mark,motif)
        sw = 2.5 if small else 2.1
        transform = 'translate(85 125) scale(3.5)' if small == 16 else 'translate(92 129) scale(3)' if small else 'translate(96 133) scale(2.65)'
        result += '<g transform="'+transform+'" fill="none" stroke="#005692" color="#005692" stroke-width="'+str(sw)+'" stroke-linecap="round" stroke-linejoin="round" opacity=".8">'+motif+'</g>'
        if not small:
            result += '<g transform="translate(96 132) scale(2.65)" fill="none" stroke="#d5ffff" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" opacity=".4">'+motif+'</g>'
    return result


def files(small):
    result = ''
    # Three distinct planes fan around a shared lower pivot.
    for angle, x, color in [(-14,56,'url(#violet)'),(13,80,'url(#ice)')]:
        body = box(x,55,113,140,14,color,'fill-opacity=".82" stroke="#deffff" stroke-opacity=".65" stroke-width="1.2"')
        result += '<g transform="rotate('+str(angle)+' 128 169)">'+shadow(box(x,55,113,140,14,'#001c51'),small)+body+'</g>'
    back = 'M46 109V96Q46 84 58 84H104L123 101H199Q211 101 211 114V188Q211 204 195 204H62Q46 204 46 188Z'
    face = 'M42 126Q41 113 55 113H202Q218 113 215 129L204 193Q202 205 189 205H67Q54 205 52 192Z'
    result += shadow(shape(back,'#002864'),small)+shape(back,'url(#ice)','stroke="#e5ffff" stroke-width="1.3"')
    result += shape(face,'url(#glass)','fill-opacity=".92" stroke="#c4ffff" stroke-width="1.5"')
    if not small:
        result += shape('M55 120H202L196 147Q129 129 58 157Z','url(#sheen)','opacity=".75"')
        result += line('M61 124H198','#fff',2,'opacity=".72"')
        result += line('M63 192Q65 198 72 198H186','#0056c0',2,'opacity=".28"')
        result += shape('M52 129L60 185Q63 199 72 199L66 139Z','#c6ffff','opacity=".35"')
        result += line('M208 130L198 186','#0064ce',3,'opacity=".35"')
    return result


def settings(small):
    result = ''
    positions = [(78,96),(128,165),(178,115)] if not small else [(78,92),(128,169),(178,112)]
    for i,(y,x) in enumerate(positions):
        result += shadow(box(48,y-11,160,25,12,'#000'),small)
        result += box(48,y-12,160,24,12,'url(#dark)','stroke="#a8b9d2" stroke-opacity=".45" stroke-width="1"')
        result += box(51,y-9,x-50,18,9,'url(#glass)' if i != 1 else 'url(#violet)','opacity=".75"')
        if not small:
            result += line(f'M59 {y-7}H197','#fff',1,'opacity=".28"')
        result += orb(x,y+2,20,'#000','opacity=".35" filter="url(#layer)"') if not small else ''
        result += orb(x,y,19 if not small else 21,'url(#edge)','stroke="#f7ffff" stroke-width="1"')
        result += orb(x,y,14 if not small else 15,'url(#ice)' if i != 1 else 'url(#violet)','stroke="#405a88" stroke-opacity=".5" stroke-width="1"')
        if not small:
            result += shape(f'M{x-10} {y-5}Q{x} {y-17} {x+11} {y-4}Q{x} {y-8} {x-10} {y-5}Z','#fff','opacity=".65"')
    return result


def generic(small):
    result = ''
    for x,y,w,h,color,angle in [(63,65,125,130,'url(#violet)',-12),(76,76,125,130,'url(#glass)',9)]:
        b = box(x,y,w,h,23,color,'stroke="#e1faff" stroke-opacity=".7" stroke-width="1.3"')
        result += '<g transform="rotate('+str(angle)+' 128 140)">'+shadow(box(x,y,w,h,23,'#001c51'),small)+b+'</g>'
    result += box(68,71,120,127,23,'url(#ice)','fill-opacity=".82" stroke="#efffff" stroke-width="1.5"')
    # An empty inset portal stands for an app that has not supplied its own identity.
    result += box(89,95,78,76,15,'url(#glass)','stroke="#f5ffff" stroke-opacity=".8" stroke-width="1.2"')
    result += box(101,107,54,52,10,'url(#paper)','opacity=".75"')
    if not small:
        result += line('M81 84Q128 75 174 88','#fff',2,'opacity=".8"')
    return result


def document(kind, small):
    page = 'M77 42H155L192 80V202Q192 214 180 214H77Q65 214 65 202V54Q65 42 77 42Z'
    result = shadow(shape(page,'#132145'),small)+shape(page,'url(#paper)','stroke="#e9f8ff" stroke-width="1.2"')
    result += shape('M155 42V69Q155 80 166 80H192Z','url(#edge)')
    if not small:
        result += line('M79 58H143M79 67H137','#d8e2ed',2)
        result += line('M79 190H174M79 198H149','#c2d5e7',2,'opacity=".65"')
    colors = {'pdf':['#ff969a','#fa3d63','#b90c3c'],'image':['#b6a7ff','#8762f3','#5038c1'],'audio':['#96ffde','#1dceae','#008c8c'],'video':['#dca5ff','#ab4ff0','#6d23b7'],'archive':['#ffe2a3','#ffb344','#ca6912'],'text':['#9bdcff','#359eee','#105ec1'],'executable':['#a4ffe3','#2ad399','#028d6b'],'generic':['#c7d7f3','#7998c5','#42628f']}
    # A coloured type slab overlaps the page, with a luminous top edge and a lower contact shadow.
    c = colors[kind]
    result += '<defs>'+gradient('badge',c,y1=98,y2=188)+'</defs>'
    bx,by,bw,bh = (87,102,116,83) if not small else (82,99,126,91)
    result += shadow(box(bx,by,bw,bh,17,'#0a2350'),small)
    result += box(bx,by,bw,bh,17,'url(#badge)','stroke="#fff" stroke-opacity=".65" stroke-width="1.2"')
    if not small:
        result += shape('M94 107H195V128Q145 112 93 139Z','url(#sheen)','opacity=".55"')
    if kind == 'pdf':
        if small == 16:
            result += line('M114 160V121H136Q151 121 151 133Q151 145 136 145H115','#fff',9)
        else:
            result += line('M103 158V127H114Q127 127 127 136Q127 145 114 145H103M136 158V127H142Q159 127 159 142Q159 158 142 158ZM171 158V127H190M171 141H186','#fff',6 if small else 5)
    elif kind == 'image':
        result += orb(166,123,10,'#fff0b1')
        result += shape('M99 168L128 126L159 168Z','#d6f9ff','opacity=".9"')
        result += shape('M130 168L164 137L193 168Z','#fff','opacity=".75"')
        if not small:
            result += shape('M128 126L140 151L128 145L117 143Z','#fff')
    elif kind == 'audio':
        for x,h in [(108,19),(126,44),(145,60),(164,34),(182,15)]:
            result += box(x,144-h/2,7 if not small else 9,h,3.5,'url(#ice)')
    elif kind == 'video':
        result += shape('M129 119Q125 117 125 123V166Q125 172 131 168L171 148Q177 144 171 141Z','url(#ice)','stroke="#fff" stroke-width="1"')
    elif kind == 'archive':
        result += box(135,102,22,83,3,'#8f4813','opacity=".5"')
        for i in range(4 if small else 6):
            result += box(133 if i%2 else 143,110+i*(14 if small else 10),12,6,2,'#fff2c5')
        result += box(135,158,22,18,4,'url(#edge)','stroke="#fff" stroke-width="1"')
        result += box(142,163,8,7,2,'#ae6e21')
    elif kind == 'executable':
        result += line('M123 126L105 144L123 162M168 126L186 144L168 162','#fff',7 if small else 5)
        result += line('M153 124L138 164','#c8fff4',6 if small else 4)
    else:
        for y,w in [(127,63),(143,73),(159,47)]:
            result += box(108,y,w,6 if not small else 8,3,'#fff','opacity=".9"')
    return result


def trash(full, small):
    result = ''
    if full:
        for x,y,angle,color in [(75,49,-14,'url(#paper)'),(120,42,17,'url(#gold)'),(101,61,-2,'url(#ice)')]:
            result += box(x,y,65,96,9,color,f'transform="rotate({angle} 128 120)" stroke="#fff" stroke-width="1.2"')
    silhouette = 'M65 83L77 190Q80 211 128 211Q176 211 179 190L191 83Z'
    result += shadow(shape(silhouette,'#005369'),small)
    result += shape(silhouette,'url(#mint)','fill-opacity=".75" stroke="#e3ffff" stroke-width="1.7"')
    result += '<ellipse cx="128" cy="83" rx="63" ry="19" fill="#126e83" opacity=".65"/>'
    result += '<ellipse cx="128" cy="81" rx="63" ry="19" fill="none" stroke="url(#ice)" stroke-width="6"/>'
    if full:
        result += shape('M87 115L114 107L133 145L104 158Z','#fff','opacity=".55"')
        result += shape('M134 131L159 117L169 159L142 174Z','#d1ffef','opacity=".45"')
    if not small:
        result += line('M79 107L89 184Q91 195 106 196','#efffff',5,'opacity=".7"')
        result += line('M179 105L170 183','#007781',4,'opacity=".35"')
        result += '<ellipse cx="128" cy="196" rx="39" ry="7" fill="#baffeb" opacity=".4"/>'
    else:
        result += line('M81 110L90 183','#ddfff6',7,'opacity=".7"')
    return result


def computer(small):
    body = box(40,53,176,128,14,'url(#dark)','stroke="url(#edge)" stroke-width="2"')
    result = shadow(box(40,53,176,128,14,'#000'),small)+body
    result += box(48,61,160,108,8,'url(#glass)')
    result += shape('M48 131Q91 82 143 118Q176 141 208 100V161Q208 169 200 169H56Q48 169 48 161Z','url(#violet)')
    result += shape('M48 146Q109 104 162 145Q187 163 208 140V163Q208 169 200 169H56Q48 169 48 161Z','url(#ice)','opacity=".65"')
    result += shape('M112 181H144L150 202H106Z','url(#edge)')
    result += shape('M97 201H159L174 211Q176 215 170 215H86Q80 215 82 211Z','url(#paper)','stroke="#b2c8e0" stroke-width="1"')
    if not small:
        result += line('M56 65H198','#fff',1,'opacity=".6"')+orb(128,175,1.6,'#d0f8ff')
    return result


def badge(locked, small):
    if locked:
        arch = 'M86 119V92Q86 50 128 50Q170 50 170 92V119H155V91Q155 66 128 66Q101 66 101 92V119Z'
        result = shadow(shape(arch,'#6c3100'),small)+shape(arch,'url(#edge)','stroke="#fff" stroke-width="1.1"')
        result += shadow(box(70,108,116,103,23,'#843800'),small)+box(70,108,116,103,23,'url(#gold)','stroke="#fff1cb" stroke-width="1.6"')
        result += orb(128,150,13 if small else 11,'#83501c')+box(123,152,10,25,5,'#83501c')
        if not small:
            result += shape('M81 117H176V140Q126 122 80 151Z','url(#sheen)')
        return result
    arrow = 'M60 188V141Q60 103 99 103H140V72Q140 63 148 69L201 114Q207 120 201 126L148 171Q140 178 140 168V137H100Q90 137 90 148V188Q90 196 82 196H68Q60 196 60 188Z'
    result = shadow(shape(arrow,'#001f40'),small)+shape(arrow,'url(#ice)','stroke="#e8ffff" stroke-width="1.5"')
    if not small:
        result += line('M70 179V141Q70 113 100 113H149V84','#fff',3,'opacity=".7"')
    return result


def render(name, small, symbols):
    if name == 'application.file-manager':
        palette, body = ['#06cafd','#0081f4','#2445d2'], files(small)
    elif name == 'application.settings':
        palette, body = ['#535c6d','#2a303d','#121722'], settings(small)
    elif name == 'application.generic':
        palette, body = ['#8a9db8','#455978','#26354e'], generic(small)
    elif name.startswith('folder'):
        mark = {'documents':'document','downloads':'download','home':'house','pictures':'photo','movies':'film','music':'music','applications':'grid','desktop':'desktop'}.get(name.split('.')[-1])
        palette, body = ['#f7fdff','#d8f1ff','#a3cced'], folder(mark,small,symbols,name.endswith('generic-open'))
    elif name.startswith('document.'):
        kind = name.split('.')[1]
        palettes = {'pdf':['#ff8b99','#ed355c','#ab1647'],'image':['#baa1ff','#8b5af1','#5735b6'],'audio':['#74edd0','#16bda8','#027f85'],'video':['#d994ff','#a343e8','#5e1dac'],'archive':['#ffe09a','#f4ab3c','#d4791e'],'text':['#88d7ff','#319ae9','#155bb5'],'executable':['#7eedbc','#22b481','#067358'],'generic':['#e6eff8','#b8cfdf','#829fb9']}
        palette, body = palettes[kind], document(kind,small)
    elif name.startswith('trash.'):
        palette, body = ['#f3fff9','#d6f3e8','#91ccbf'], trash(name.endswith('full'),small)
    elif name == 'computer':
        palette, body = ['#8276f3','#4945b1','#272768'], computer(small)
    elif name == 'locked.badge':
        palette, body = ['#ffdb7b','#f4a82e','#c77215'], badge(True,small)
    else:
        palette, body = ['#469cae','#225e79','#12394e'], badge(False,small)
    return '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">'+definitions(palette)+tile(small)+body+'</svg>\n'
