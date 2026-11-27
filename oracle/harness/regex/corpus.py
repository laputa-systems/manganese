import random
import struct

ALPHABET = list('abcXYZ019_ -./')
SUBJECT_ATOMS = list('abcXYZ019_ -./\néΩ\u03a9\u4e2d')
LITERALS = ['a', 'b', 'c', 'X', 'Y', '0', '1', '9', '_', '-', '.', '/', 'é', 'Ω', '中']
CLASSES = ['[abc]', '[^ab]', '[a-z]', '[A-Z]', '[0-9]', '[a-c0-2]', '[\\dA-F]', '[\\w-]', '[.\\-]']
SCLASS = ['\\d', '\\D', '\\w', '\\W', '\\s', '\\S']
ESCAPES = ['\\n', '\\t', '\\x41', '\\x2e', '\\u0061', '\\u00e9', '\\u03a9', '\\.', '\\*', '\\(', '\\[', '\\\\']
ANCHORS = ['^', '$']
QUANTIFIERS = ['', '', '', '?', '*', '+', '??', '*?', '+?', '{2}', '{1,3}', '{0,2}', '{2,}', '{1,2}?']
FLAG_SET = ['g', 'i', 'm', 's', 'u']
NAMES = ['w', 'x', 'g1']


def encode_request(pattern, flags, subject, iterations=2, start=0):
    p = pattern.encode('utf-8')
    f = flags.encode('ascii')
    s = subject.encode('utf-8')
    if len(p) > 65535 or len(f) > 65535 or len(s) > 65535:
        raise ValueError('request field too long')
    return (bytes([1]) + struct.pack('>H', len(p)) + p + struct.pack('>H', len(f)) + f +
            struct.pack('>H', len(s)) + s + struct.pack('>HH', iterations, start))


def decode_request(data):
    if not data or data[0] != 1:
        raise ValueError('version')
    offset = 1
    fields = []
    for _ in range(3):
        if offset + 2 > len(data):
            raise ValueError('length')
        size = struct.unpack_from('>H', data, offset)[0]
        offset += 2
        if offset + size > len(data):
            raise ValueError('field')
        fields.append(data[offset:offset + size].decode('utf-8'))
        offset += size
    if offset + 4 != len(data):
        raise ValueError('tail')
    iterations, start = struct.unpack_from('>HH', data, offset)
    return fields + [iterations, start]


def atom(rng):
    choice = rng.randrange(9)
    if choice == 0:
        return rng.choice(LITERALS), True
    if choice == 1:
        return rng.choice(CLASSES), True
    if choice == 2:
        return rng.choice(SCLASS), True
    if choice == 3:
        return rng.choice(ESCAPES), True
    if choice == 4:
        return '.', True
    if choice == 5:
        return rng.choice(ALPHABET), True
    if choice == 6:
        return rng.choice(ESCAPES), True
    if choice == 7:
        return rng.choice(['(', '(?:']) + branch(rng) + ')', False
    return '(?:' + branch(rng) + ')', False


def piece(rng):
    base, quantifiable = atom(rng)
    return base + rng.choice(QUANTIFIERS) if quantifiable else base


def branch(rng):
    pieces = [piece(rng) for _ in range(rng.randrange(1, 4))]
    if rng.random() < 0.25:
        pieces.append('|')
        pieces.extend(piece(rng) for _ in range(rng.randrange(1, 3)))
    return ''.join(pieces)


def flags(rng):
    return ''.join(flag for flag in FLAG_SET if rng.random() < 0.35)


def subject(rng):
    if rng.random() < 0.25:
        return ''.join(rng.choice('abcXYZ019_ -./') for _ in range(rng.randrange(0, 14)))
    text = ''.join(rng.choice(SUBJECT_ATOMS) for _ in range(rng.randrange(0, 18)))
    while text.endswith('\n'):
        text = text[:-1]
    return text


FIXED = [
    ('(a)(b)', '', 'zabab'),
    ('a', 'g', 'aba'),
    ('', 'g', 'abc'),
    ('a*', 'gd', 'aa'),
    ('[a-z]+', 'gi', 'AbC'),
    ('^b', 'm', 'a\nb\nc'),
    ('a.$', 's', 'a\n'),
    ('(?<w>\\w+)', 'gu', 'hey there'),
    ('é', 'iu', 'Éé'),
    ('\\u00e9', '', 'é'),
    ('a|ab', '', 'ab'),
    ('ab|a', '', 'ab'),
    ('(a+)(a)', '', 'aaa'),
    ('a??b', '', 'b'),
    ('[0-9]{2,3}', 'g', 'a1234b'),
    ('\\s+', 'g', 'a \t b'),
    ('\\D+', '', '12ab34'),
    ('x.y', '', 'x\ny'),
    ('x.y', 's', 'x\ny'),
    ('(a)?b', '', 'b'),
    ('(a)|(b)', '', 'b'),
    ('a{0}', '', 'aaa'),
]

INVALID = ['(', '[', '*a', 'a{3,2}', ')', '\\', '(?<1a>x)', 'a**', '[z-a]']


def random_case(rng):
    pattern = branch(rng)
    if rng.random() < 0.12:
        pattern = rng.choice(INVALID)
    return pattern, flags(rng), subject(rng), rng.randrange(1, 5), 0


def cases(lane, count):
    rng = random.Random(991 if lane == 'seen' else 992)
    result = []
    for pattern, flag, text in FIXED:
        for variant in range(3):
            iterations = 1 + variant
            subject_text = text if lane == 'seen' else text + 'q'
            result.append(encode_request(pattern, flag, subject_text, iterations, 0))
    for pattern in INVALID:
        result.append(encode_request(pattern, 'u', 'abc', 2, 0))
    while len(result) < count:
        result.append(encode_request(*random_case(rng)))
    return result[:count]
