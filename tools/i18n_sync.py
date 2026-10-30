"""Keeps translation files in step with the code. The key is the Russian text itself.

python tools/i18n_sync.py          adds missing strings to assets/i18n/*.toml, drops unused ones
python tools/i18n_sync.py --check  fails if a string has no translation (for CI)

A new language: copy en.toml to de.toml, change _name, translate the right-hand side.
"""
import glob
import json
import os
import re
import sys

ROOT = os.path.join(os.path.dirname(__file__), '..')
CYR = re.compile('[А-Яа-яЁё]')
CALL = re.compile(r'\b(?:t|tf|n)\(\s*$')


def tokens(src):
    """Yields (start, end, value) of every string literal, skipping comments and char literals."""
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if src.startswith('//', i):
            i = src.find('\n', i)
            if i < 0:
                return
        elif src.startswith('/*', i):
            i = src.index('*/', i) + 2
        elif c == 'r' and re.match(r'r#*"', src[i:i + 8]) and not src[i - 1].isalnum():
            hashes = re.match(r'r(#*)"', src[i:]).group(1)
            end = src.index('"' + hashes, i + 2 + len(hashes))
            yield i, end + 1 + len(hashes), src[i + 2 + len(hashes):end]
            i = end + 1 + len(hashes)
        elif c == '"':
            j = i + 1
            out = []
            while src[j] != '"':
                if src[j] == '\\':
                    e = src[j + 1]
                    if e == '\n':
                        j += 2
                        while src[j] in ' \n\r\t':
                            j += 1
                        continue
                    out.append({'n': '\n', 't': '\t', '"': '"', '\\': '\\', 'r': '\r'}.get(e, e))
                    j += 2
                else:
                    out.append(src[j])
                    j += 1
            yield i, j + 1, ''.join(out)
            i = j + 1
        elif c == "'" and re.match(r"'(\\.|[^\\'])'", src[i:i + 4]):
            i += re.match(r"'(\\.|[^\\'])'", src[i:i + 4]).end()
        else:
            i += 1


def code_part(path):
    src = open(path, encoding='utf-8').read()
    cut = src.find('#[cfg(test)]')
    return src if cut < 0 else src[:cut]


def strings():
    found = []
    for path in sorted(glob.glob(os.path.join(ROOT, 'crates', '*', 'src', '*.rs'))):
        src = code_part(path)
        for a, _, value in tokens(src):
            if CYR.search(value) and CALL.search(src[:a]) and value not in found:
                found.append(value)
    return found


def quote(s):
    return json.dumps(s, ensure_ascii=False)


def read_file(path):
    name, items = '', {}
    for line in open(path, encoding='utf-8'):
        m = re.match(r'^("(?:[^"\\]|\\.)*") = ("(?:[^"\\]|\\.)*")\s*$', line)
        if m:
            items[json.loads(m.group(1))] = json.loads(m.group(2))
        elif line.startswith('_name'):
            name = json.loads(line.split('=', 1)[1])
    return name, items


def main():
    wanted = strings()
    check = '--check' in sys.argv
    bad = 0
    for path in sorted(glob.glob(os.path.join(ROOT, 'assets', 'i18n', '*.toml'))):
        name, items = read_file(path)
        missing = [s for s in wanted if not items.get(s)]
        if check:
            for s in missing:
                print(f'{os.path.basename(path)}: {s}')
            bad += len(missing)
            continue
        with open(path, 'w', encoding='utf-8', newline='\n') as f:
            f.write(f'_name = {quote(name)}\n\n')
            for s in wanted:
                f.write(f'{quote(s)} = {quote(items.get(s, ""))}\n')
        print(os.path.basename(path), len(wanted), 'strings,', len(missing), 'to translate')
    sys.exit(1 if bad else 0)


if __name__ == '__main__':
    main()
