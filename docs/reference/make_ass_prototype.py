"""Prototype: niconico-style comments (JSON) -> ASS with scrolling danmaku.

Throwaway sample for the round 3 decision sheet. The real converter will live in the
Rust core crate (`comments/`). Input mirrors the fields of niconico's nvComment threads
that yt-dlp saves as the `comments` subtitle: vposMs, body, commands.

Usage: python make_ass.py comments.json out.ass [--size 1.0] [--opacity 100]
       [--max-on-screen 0] [--font "BIZ UDPGothic"]
"""
import argparse
import json
import math

W, H = 1280, 720
SCROLL_SEC = 4.0          # time a scrolling comment takes to cross the screen
FIXED_SEC = 3.0           # ue / shita comments stay this long
GAP_PX = 24               # minimum horizontal gap between comments in one lane

COLORS = {  # niconico command colours as RGB
    'white': 0xFFFFFF, 'red': 0xFF0000, 'pink': 0xFF8080, 'orange': 0xFFC000,
    'yellow': 0xFFFF00, 'green': 0x00FF00, 'cyan': 0x00FFFF, 'blue': 0x0000FF,
    'purple': 0xC000FF, 'black': 0x000000,
}
SIZES = {'small': 0.65, 'medium': 1.0, 'big': 1.6}


def ass_time(sec):
    cs = int(round(sec * 100))
    h, cs = divmod(cs, 360000)
    m, cs = divmod(cs, 6000)
    s, cs = divmod(cs, 100)
    return f'{h}:{m:02d}:{s:02d}.{cs:02d}'


def ass_color(rgb):
    r, g, b = (rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255
    return f'&H{b:02X}{g:02X}{r:02X}&'


def text_width(text, font_px):
    """Rough width: full-width characters count 1 em, others 0.55 em."""
    em = sum(1.0 if ord(ch) >= 0x2E80 else 0.55 for ch in text)
    return em * font_px


def escape(text):
    return text.replace('\\', '\\\\').replace('{', '\\{').replace('}', '\\}').replace('\n', '\\N')


def build(comments, size=1.0, opacity=100, max_on_screen=0, font='BIZ UDPGothic'):
    base_px = H * 0.0625 * size            # medium comment: 45 px at 720p, size 1.0
    line_h = base_px * 1.25
    lanes = max(1, int(H // line_h))
    scroll_lanes = [None] * lanes           # last (start, width, speed) per lane
    top_free = [0.0] * lanes                # time each ue lane frees up
    bottom_free = [0.0] * lanes             # time each shita lane frees up
    alpha = f'&H{round((100 - opacity) * 255 / 100):02X}&'
    shown, dropped, events = [], 0, []

    for c in sorted(comments, key=lambda c: c['vposMs']):
        start = c['vposMs'] / 1000
        cmds = set(c.get('commands', []))
        px = base_px * next((SIZES[s] for s in SIZES if s in cmds), 1.0)
        rgb = next((COLORS[k] for k in COLORS if k in cmds), COLORS['white'])
        body = c['body']

        if max_on_screen:
            shown = [e for e in shown if e > start]
            if len(shown) >= max_on_screen:
                dropped += 1
                continue

        tags = f'\\fs{px:.0f}\\c{ass_color(rgb)}\\alpha{alpha}'
        if 'ue' in cmds or 'shita' in cmds:
            # As on niconico: ue stacks down from the top edge, shita up from the bottom
            # edge, drawn above scrolling comments (layer 1) rather than moved aside.
            top = 'ue' in cmds
            free = top_free if top else bottom_free
            lane = next((i for i, t in enumerate(free) if t <= start), None)
            if lane is None:
                dropped += 1
                continue
            free[lane] = start + FIXED_SEC
            end = start + FIXED_SEC
            if top:
                pos = f'\\an8\\pos({W // 2},{lane * line_h:.0f})'
            else:
                pos = f'\\an2\\pos({W // 2},{H - lane * line_h:.0f})'
            events.append((start, end, f'{{{pos}{tags}}}{escape(body)}', 1))
        else:
            width = text_width(body, px)
            speed = (W + width) / SCROLL_SEC
            busy = {i for i, t in enumerate(top_free) if t > start}
            busy |= {lanes - 1 - i for i, t in enumerate(bottom_free) if t > start}
            lane = None
            for i, prev in enumerate(scroll_lanes):
                if i in busy:
                    continue
                if prev is None:
                    lane = i
                    break
                p_start, p_width, p_speed = prev
                entered = p_start + (p_width + GAP_PX) / p_speed
                no_catch_up = p_start + SCROLL_SEC - W / speed
                if start >= max(entered, no_catch_up):
                    lane = i
                    break
            if lane is None:
                dropped += 1
                continue
            scroll_lanes[lane] = (start, width, speed)
            y = lane * line_h
            end = start + SCROLL_SEC
            move = f'\\an7\\move({W},{y:.0f},{-math.ceil(width)},{y:.0f})'
            events.append((start, end, f'{{{move}{tags}}}{escape(body)}', 0))
        shown.append(end)

    header = '\n'.join([
        '[Script Info]',
        'ScriptType: v4.00+',
        f'PlayResX: {W}',
        f'PlayResY: {H}',
        'WrapStyle: 2',
        'ScaledBorderAndShadow: yes',
        '',
        '[V4+ Styles]',
        'Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, '
        'BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, '
        'BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding',
        f'Style: Danmaku,{font},{base_px:.0f},&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,'
        '-1,0,0,0,100,100,0,0,1,2,0,7,0,0,0,1',
        '',
        '[Events]',
        'Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text',
    ])
    # Fixed (ue/shita) comments sit on layer 1, above scrolling ones, as on niconico.
    lines = [f'Dialogue: {layer},{ass_time(s)},{ass_time(e)},Danmaku,,0,0,0,,{t}'
             for s, e, t, layer in events]
    return header + '\n' + '\n'.join(lines) + '\n', len(events), dropped


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('comments')
    ap.add_argument('out')
    ap.add_argument('--size', type=float, default=1.0)
    ap.add_argument('--opacity', type=int, default=100)
    ap.add_argument('--max-on-screen', type=int, default=0)
    ap.add_argument('--font', default='BIZ UDPGothic')
    a = ap.parse_args()
    comments = json.load(open(a.comments, encoding='utf-8'))
    text, kept, dropped = build(comments, a.size, a.opacity, a.max_on_screen, a.font)
    with open(a.out, 'w', encoding='utf-8-sig') as f:
        f.write(text)
    print(f'{a.out}: {kept} comments placed, {dropped} dropped')


if __name__ == '__main__':
    main()
