"""Generate crates/core/tests/fixtures/naming/cases.json from real yt-dlp output.

Each case holds a name template in the app's model, a context, video metadata, the yt-dlp
output template the app must compile to for that video (names are compiled per job, after
probing), and the path yt-dlp itself produces from it.
Run on Windows with yt-dlp installed:

    PYTHONIOENCODING=utf-8 python tools/gen_naming_fixture.py

The reference compiler below defines the expected `outtmpl`; the Rust implementation must
produce the same string. The expected paths come from yt-dlp's prepare_filename, not from
this script.
"""
import json
import pathlib
import sys

import yt_dlp
from yt_dlp import YoutubeDL
from yt_dlp.utils import sanitize_filename

OUT = pathlib.Path(__file__).resolve().parents[1] / 'crates/core/tests/fixtures/naming/cases.json'

EXPR = {
    'uploader': 'uploader,channel,uploader_id',
    'channel': 'channel,uploader',
    'title': 'title',
    'id': 'id',
    'playlist': 'playlist_title,playlist',
    'index': 'playlist_index',
    'series': 'series',
    'episode_number': 'episode_number',
    'episode': 'episode',
}


def date_expr(offset):
    if offset == 0:
        ts = 'timestamp'
    elif offset > 0:
        ts = f'timestamp+{offset}'
    else:
        ts = f'timestamp-{-offset}'
    return f'{ts}>%Y-%m-%d,upload_date>%Y-%m-%d'


def expr(token, ctx):
    return date_expr(ctx['tz_offset_seconds']) if token == 'date' else EXPR[token]


def compile_part(part, ctx):
    if 'text' in part:
        return part['text'].replace('%', '%%')
    if 'field' in part:
        token = part['field']
        if token == 'site':
            return sanitize_filename(ctx['site_name']).replace('%', '%%')
        if token == 'title' and ctx.get('title_budget_bytes') is not None:
            return f'%({expr(token, ctx)}|).{ctx["title_budget_bytes"]}B'
        return f'%({expr(token, ctx)}|)s'
    opt = part['optional']
    token = opt['field']
    replacement = f'{opt["before"]}{{}}{opt["after"]}'
    return f'%({expr(token, ctx)}&{replacement}|)s'


ALTERNATIVES = {
    'uploader': ['uploader', 'channel', 'uploader_id'],
    'channel': ['channel', 'uploader'],
    'date': ['timestamp', 'upload_date'],
    'title': ['title'],
    'id': ['id'],
    'playlist': ['playlist_title', 'playlist'],
    'index': ['playlist_index'],
    'series': ['series'],
    'episode_number': ['episode_number'],
    'episode': ['episode'],
}


def has_value(token, info):
    if token == 'site':
        return True
    return any(info.get(k) not in (None, '') for k in ALTERNATIVES[token])


def folder_is_empty(folder, info):
    """A folder whose every part would render empty is left out, so no path starts at
    the drive root (yt-dlp turns a leading empty folder into a leading separator)."""
    for part in folder:
        if 'text' in part and part['text']:
            return False
        token = part.get('field') or part.get('optional', {}).get('field')
        if token and has_value(token, info):
            return False
    return True


def compile_template(tmpl, ctx, info):
    folders = [''.join(compile_part(p, ctx) for p in folder)
               for folder in tmpl['folders'] if not folder_is_empty(folder, info)]
    file = ''.join(compile_part(p, ctx) for p in tmpl['file']) + '.%(ext)s'
    return '/'.join(folders + [file])


F = lambda name: {'field': name}  # noqa: E731
T = lambda text: {'text': text}  # noqa: E731
OPT = lambda name, before='', after='': {'optional': {'before': before, 'field': name, 'after': after}}  # noqa: E731

DEFAULT = {'folders': [[F('uploader')]],
           'file': [OPT('date', after=' '), F('title'), T(' ['), F('id'), T(']')]}
SITE_FIRST = {'folders': [[F('site')], [F('uploader')]],
              'file': [OPT('date', after=' '), F('title'), T(' ['), F('id'), T(']')]}
FLAT = {'folders': [], 'file': [OPT('date', after=' '), F('title'), T(' ['), F('id'), T(']')]}
EPISODE = {'folders': [], 'file': [F('title'), OPT('episode', before=' - '), T(' ['), F('id'), T(']')]}
LITERAL_PERCENT = {'folders': [], 'file': [T('100% '), F('title')]}
SERIES_FOLDER = {'folders': [[OPT('series')]],
                 'file': [OPT('episode_number', before='第', after='話 '), F('title')]}

SM9 = {'id': 'sm9', 'title': '新・豪血寺一族 -煩悩解放 - レッツゴー！陰陽師', 'uploader': '中の',
       'channel': '中の', 'uploader_id': '4', 'timestamp': 1173108780, 'upload_date': '20070305',
       'extractor': 'niconico', 'ext': 'mp4'}
NL = {'id': 'nl1872567', 'title': '【12/25放送分】『生対談!!ひろゆきと戀塚のニコニコを作った人 』前半',
      'uploader': 'nicolive', 'channel': 'nicolive', 'timestamp': 1198637246,
      'upload_date': '20071226', 'extractor': 'niconico', 'ext': 'mp4'}
BBB = {'id': 'YE7VzlLtp-4', 'title': 'Big Buck Bunny', 'uploader': 'Blender', 'channel': 'Blender',
       'uploader_id': '@BlenderOfficial', 'timestamp': 1212060266, 'upload_date': '20080529',
       'extractor': 'youtube', 'ext': 'mp4'}


def ctx(offset=32400, site='ニコニコ動画', budget=None):
    return {'tz_offset_seconds': offset, 'site_name': site, 'title_budget_bytes': budget}


def without(info, *keys):
    return {k: v for k, v in info.items() if k not in keys}


CASES = [
    ('default_jst', DEFAULT, ctx(), SM9),
    ('default_utc', DEFAULT, ctx(0), SM9),
    ('default_minus5h', DEFAULT, ctx(-18000), SM9),
    ('no_timestamp_uses_upload_date', DEFAULT, ctx(), without(SM9, 'timestamp')),
    ('no_date_drops_with_space', DEFAULT, ctx(), without(SM9, 'timestamp', 'upload_date')),
    ('slash_in_title', DEFAULT, ctx(), NL),
    ('youtube', DEFAULT, ctx(site='YouTube'), BBB),
    ('site_folder', SITE_FIRST, ctx(), SM9),
    ('flat', FLAT, ctx(), SM9),
    ('forbidden_chars', FLAT, ctx(), dict(SM9, title='a"b*c:d<e>f?g|h/i\\j')),
    ('clock_time_colon', FLAT, ctx(), dict(SM9, title='Live 12:30 start, ratio a:b')),
    ('newline_and_control', FLAT, ctx(), dict(SM9, title='line1\nline2\x07end')),
    ('emoji', FLAT, ctx(), dict(SM9, title='テスト🎬動画')),
    ('budget_60_bytes', DEFAULT, ctx(budget=60), SM9),
    ('budget_splits_multibyte', FLAT, ctx(budget=8), dict(SM9, title='あいうえお')),
    ('budget_ascii', FLAT, ctx(budget=10), dict(SM9, title='abcdefghijklmnopqrstuvwxyz')),
    ('budget_counts_raw_bytes', FLAT, ctx(budget=12), dict(SM9, title='12/25/2007 special')),
    ('episode_present', EPISODE, ctx(), dict(SM9, episode='第1話 はじまり')),
    ('episode_absent', EPISODE, ctx(), SM9),
    ('uploader_falls_back_to_channel', DEFAULT, ctx(), dict(without(SM9, 'uploader'), channel='チャンネル名')),
    ('uploader_falls_back_to_id', DEFAULT, ctx(), without(SM9, 'uploader', 'channel')),
    ('no_uploader_no_folder', DEFAULT, ctx(), without(SM9, 'uploader', 'channel', 'uploader_id')),
    ('literal_percent', LITERAL_PERCENT, ctx(), SM9),
    ('series_and_episode_number', SERIES_FOLDER, ctx(), dict(SM9, series='シリーズ名', episode_number=3)),
    ('series_missing', SERIES_FOLDER, ctx(), SM9),
    ('folder_trailing_dot_windows', DEFAULT, ctx(), dict(SM9, uploader='Mr. Dot.')),
]


def main():
    if sys.platform != 'win32':
        sys.exit('Run on Windows: the expected paths include Windows path sanitizing.')
    cases = []
    with YoutubeDL({'quiet': True, 'no_warnings': True}) as ydl:
        for name, tmpl, context, info in CASES:
            outtmpl = compile_template(tmpl, context, info)
            ydl.params['outtmpl'] = {'default': outtmpl}
            path = ydl.prepare_filename(dict(info))
            if path is None:
                sys.exit(f'yt-dlp rejected the template of case {name}: {outtmpl}')
            cases.append({'name': name, 'template': tmpl, 'context': context, 'info': info,
                          'outtmpl': outtmpl, 'windows_path': path.replace('\\', '/')})
    OUT.write_text(json.dumps({
        'generator': f'yt-dlp {yt_dlp.version.__version__} prepare_filename on Windows',
        'path_separator_note': 'windows_path uses "/" between components; yt-dlp printed "\\\\"',
        'cases': cases,
    }, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(f'{len(cases)} cases -> {OUT}')


if __name__ == '__main__':
    main()
