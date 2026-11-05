# yt-dlp naming and filter rules (reference for crates/core)

Copied from yt-dlp 2026.07.04 (`yt_dlp/utils/_utils.py`), released under the Unlicense.
The Rust port in `crates/core/src/naming.rs` must behave the same; the fixture
`crates/core/tests/fixtures/naming/cases.json` holds real yt-dlp output and is the arbiter.

## Order in which yt-dlp evaluates one `%(...)` field (verified offline, 2026-11-02)
1. Alternatives `a,b,c`: the first one that is present and not empty. Arithmetic and date
   formatting apply per alternative: `timestamp+32400>%Y-%m-%d` formats the shifted epoch
   as a UTC calendar date.
2. Replacement `&text`: when a value exists, `text` is used instead, formatted with Python
   `str.format`, so `{}` is the value (already date-formatted).
3. Default `|text`: used when no alternative has a value (`|` alone means empty).
4. Conversion: `.NB` truncates the raw value to N UTF-8 bytes and drops a partial
   character at the end. Truncation happens BEFORE sanitizing, so `/` (1 byte) counts as
   1 byte even though it becomes `⧸` (3 bytes) afterwards.
5. `sanitize_filename(value)` with the default rules below (not restricted, `is_id` unset).
6. Literal template text is copied as is; `%%` is a literal `%`; `/` separates folders.
7. On Windows only, `sanitize_path` then rewrites each path component (a trailing dot or
   space becomes `#`, and so on). macOS gets no path rewriting.
8. `upload_date` is derived by yt-dlp from `timestamp` in UTC.

A `(` or `)` inside a replacement breaks yt-dlp's template parser (it raised an error for
`%(series& ({})|)s`), so replacements must not contain `(){}|&%` or path separators.

## sanitize_filename and sanitize_path (yt-dlp source)
```python
def sanitize_filename(s, restricted=False, is_id=NO_DEFAULT):
    """Sanitizes a string so it could be used as part of a filename.
    @param restricted   Use a stricter subset of allowed characters
    @param is_id        Whether this is an ID that should be kept unchanged if possible.
                        If unset, yt-dlp's new sanitization rules are in effect
    """
    if s == '':
        return ''

    def replace_insane(char):
        if restricted and char in ACCENT_CHARS:
            return ACCENT_CHARS[char]
        elif not restricted and char == '\n':
            return '\0 '
        elif is_id is NO_DEFAULT and not restricted and char in '"*:<>?|/\\':
            # Replace with their full-width unicode counterparts
            return {'/': '\u29F8', '\\': '\u29f9'}.get(char, chr(ord(char) + 0xfee0))
        elif char == '?' or ord(char) < 32 or ord(char) == 127:
            return ''
        elif char == '"':
            return '' if restricted else '\''
        elif char == ':':
            return '\0_\0-' if restricted else '\0 \0-'
        elif char in '\\/|*<>':
            return '\0_'
        if restricted and (char in '!&\'()[]{}$;`^,#' or char.isspace() or ord(char) > 127):
            return '' if unicodedata.category(char)[0] in 'CM' else '\0_'
        return char

    # Replace look-alike Unicode glyphs
    if restricted and (is_id is NO_DEFAULT or not is_id):
        s = unicodedata.normalize('NFKC', s)
    s = re.sub(r'[0-9]+(?::[0-9]+)+', lambda m: m.group(0).replace(':', '_'), s)  # Handle timestamps
    result = ''.join(map(replace_insane, s))
    if is_id is NO_DEFAULT:
        result = re.sub(r'(\0.)(?:(?=\1)..)+', r'\1', result)  # Remove repeated substitute chars
        STRIP_RE = r'(?:\0.|[ _-])*'
        result = re.sub(f'^\0.{STRIP_RE}|{STRIP_RE}\0.$', '', result)  # Remove substitute chars from start/end
    result = result.replace('\0', '') or '_'

    if not is_id:
        while '__' in result:
            result = result.replace('__', '_')
        result = result.strip('_')
        # Common case of "Foreign band name - English song title"
        if restricted and result.startswith('-_'):
            result = result[2:]
        if result.startswith('-'):
            result = '_' + result[len('-'):]
        result = result.lstrip('.')
        if not result:
            result = '_'
    return result


def _sanitize_path_parts(parts):
    sanitized_parts = []
    for part in parts:
        if not part or part == '.':
            continue
        elif part == '..':
            if sanitized_parts and sanitized_parts[-1] != '..':
                sanitized_parts.pop()
            else:
                sanitized_parts.append('..')
            continue
        # Replace invalid segments with `#`
        # - trailing dots and spaces (`asdf...` => `asdf..#`)
        # - invalid chars (`<>` => `##`)
        sanitized_part = re.sub(r'[/<>:"\|\\?\*]|[\s.]$', '#', part)
        sanitized_parts.append(sanitized_part)

    return sanitized_parts


def sanitize_path(s, force=False):
    """Sanitizes and normalizes path on Windows"""
    if sys.platform != 'win32':
        if not force:
            return s
        root = '/' if s.startswith('/') else ''
        path = '/'.join(_sanitize_path_parts(s.split('/')))
        return root + path if root or path else '.'

    normed = s.replace('/', '\\')

    if normed.startswith('\\\\'):
        # UNC path (`\\SERVER\SHARE`) or device path (`\\.`, `\\?`)
        parts = normed.split('\\')
        root = '\\'.join(parts[:4]) + '\\'
        parts = parts[4:]
    elif normed[1:2] == ':':
        # absolute path or drive relative path
        offset = 3 if normed[2:3] == '\\' else 2
        root = normed[:offset]
        parts = normed[offset:].split('\\')
    else:
        # relative/drive root relative path
        root = '\\' if normed[:1] == '\\' else ''
        parts = normed.split('\\')

    path = '\\'.join(_sanitize_path_parts(parts))
    return root + path if root or path else '.'
```
`NO_DEFAULT` is a sentinel object, so with the defaults `is_id is NO_DEFAULT` is true and
`not is_id` is false (the `__` collapsing block does not run).

## --match-filters (yt-dlp README)
```
    --match-filters FILTER          Generic video filter. Any "OUTPUT TEMPLATE"
                                    field can be compared with a number or a
                                    string using the operators defined in
                                    "Filtering Formats". You can also simply
                                    specify a field to match if the field is
                                    present, use "!field" to check if the field
                                    is not present, and "&" to check multiple
                                    conditions. Use a "\" to escape "&" or
                                    quotes if needed. If used multiple times,
                                    the filter matches if at least one of the
                                    conditions is met. E.g. --match-filters
                                    !is_live --match-filters "like_count>?100 &
                                    description~='(?i)\bcats \& dogs\b'" matches
                                    only videos that are not live OR those that
                                    have a like count more than 100 (or the like
                                    field is not available) and also has a
                                    description that contains the phrase "cats &
                                    dogs" (caseless). Use "--match-filters -" to
                                    interactively ask whether to download each
                                    video
```

From "Filtering Formats": numeric operators `<`, `<=`, `>`, `>=`, `=`, `!=`; string
operators `=`, `^=` (starts with), `$=` (ends with), `*=` (contains), `~=` (regex). Any
string comparison may be prefixed with `!` to negate it. A comparand must be quoted with
single or double quotes if it contains spaces or characters other than `._-`. A value that
is not known fails the comparison unless the operator is followed by `?` (for example
`duration <? 600`). Conditions inside one filter are joined with `&`; repeating
`--match-filters` ORs whole filters.
