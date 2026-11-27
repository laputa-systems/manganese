import random
import struct


def u32(value):
    return struct.pack('>I', value)


def blob(value):
    if isinstance(value, str):
        value = value.encode()
    return u32(len(value)) + value


def json_request(options, text):
    if isinstance(text, str):
        text = text.encode()
    return b'\x01' + bytes([options]) + u32(len(text)) + text


def write_request(options, value):
    return b'\x02' + bytes([options]) + value


def xml_request(text, options=0):
    if isinstance(text, str):
        text = text.encode()
    return b'\x03' + u32(options) + u32(len(text)) + text


def jnull():
    return b'\x00'


def jbool(value):
    return b'\x02' if value else b'\x01'


def jint(value):
    return b'\x03' + struct.pack('>i', value)


def jdouble(value):
    return b'\x04' + struct.pack('>d', value)


def jstring(value):
    return b'\x05' + blob(value)


def jlist(values):
    return b'\x06' + u32(len(values)) + b''.join(values)


def jdict(values):
    return b'\x07' + u32(len(values)) + b''.join(blob(key) + value for key, value in values)


JSON_TEXTS = [
    'null', 'true', 'false', '0', '-0', '1', '-1', '2147483647', '2147483648',
    '-2147483648', '-2147483649', '9007199254740993', '1.0', '1.5', '-0.0',
    '1e300', '1e-300', '1E+4', '0.000001', '123456789012345678901234567890',
    '""', '"a"', '"<script>"', '"\\u003C"', '"\\b\\f\\n\\r\\t"', '"\\u0000"',
    '"\\u2028\\u2029"', '"\\uD83D\\uDE00"', '"\\ud800"', '"\\udc00"', '"\\ud800\\ud800"',
    '"\\x41"', '"\\v"', '"a\nb"', '"a\rb"', '"\x01"', '"é"', '"中"',
    '[]', '{}', '[1,2,3]', '{"a":1}', '{"b":1,"a":2}', '{"a":1,"a":2}',
    '[[[[1]]]]', '{"a":{"b":[true,false,null]}}', ' 42 ', '\ufeff42',
    '01', '1.', '.1', '+1', '1e', '1e+', 'NaN', 'Infinity', 'undefined',
    '[1,]', '{"a":1,}', '{/*c*/"a":1}', '{//c\n"a":1}', '"a\\q"', '"unterminated',
    '{"a":}', '[1,,2]', '{"a":1} trailing', '[' + '1,' * 199 + '1]',
    '[' * 201 + '0' + ']' * 201, '"\\uZZZZ"', '\xff', '{"a":1,"b":[2,{"c":3}]}',
]

WRITE_VALUES = [
    jnull(), jbool(True), jbool(False), jint(0), jint(2147483647), jint(-2147483648),
    jdouble(1.0), jdouble(-0.0), jdouble(1.5), jdouble(1e300), jdouble(1e-300),
    jstring(''), jstring('a'), jstring('<script>'), jstring('\u2028\u2029'),
    jstring('quote"backslash\\'), jstring('\x00\x01\x1f'), jstring('é中😀'),
    jlist([]), jdict([]), jlist([jint(1), jbool(False), jstring('x'), jnull()]),
    jdict([('b', jint(1)), ('a', jstring('<'))]),
    jdict([('z', jlist([jdict([('y', jdouble(2.5))])]))]),
    jlist([jdouble(1.0), jdouble(-0.0), jdouble(1e21)]),
]

XML_TEXTS = [
    '<a/>', '<a></a>', '<a>t</a>', '<a x="1"/>', '<a x="1">t</a>',
    '<a x="1" y="2"><b>t</b></a>', '<a><b/><c/></a>',
    '<a xmlns="urn:default"><b>t</b></a>',
    '<a xmlns:p="urn:x"><p:b x="1">t</p:b></a>',
    '<a xmlns:p="urn:x" xmlns:q="urn:y"><p:b q:x="1"/></a>',
    '<a>&amp;&lt;&gt;&quot;&apos;&#65;&#x42;</a>',
    '<a x="&amp;&lt;&gt;&quot;&#65;">t</a>',
    '<a><!-- comment --></a>', '<a><![CDATA[x<y&z]]></a>',
    '<?pi data?><a/>', '<a><?pi data?></a>', '<?xml version="1.0"?><a/>',
    '<a>line\nline2</a>', '<a x="tab\there"/>', '<a>é中</a>',
    '<a><b><c><d>deep</d></c></b></a>', '<root><item id="1"/><item id="2"/></root>',
    '<a><b></a>', '<a x=1/>', '<a>&bad;</a>', '<a>x</b>', '<a><![CDATA[x]]',
    '<a xmlns:p="urn:x"><q:b/></a>', '<a></a></a>', '', 'text',
    '<a><!-- b -- c --></a>', '<a x="&unknown;"/>',
]

XML_ERRORS = [
    '<a><b></a>', '<a x=1/>', '<a>&bad;</a>', '<a>x</b>', '<a><![CDATA[x]]',
    '<a xmlns:p="urn:x"><q:b/></a>', '<a></a></a>', '', 'text', '<', '<a x="',
    '<a><!-- b -- c --></a>', '<a x="&unknown;"/>', '<a>&</a>', '</a>',
]


def random_json(rng):
    alphabet = '0123456789+-.eE truefalsnul{}[],:"\\ \t\r\n<é😀'
    text = ''.join(rng.choice(alphabet) for _ in range(rng.randrange(0, 40)))
    if rng.randrange(4) == 0:
        text = rng.choice(['', ' ', '\ufeff']) + text
    return json_request(rng.randrange(64), text)


def random_value(rng, depth=0):
    kind = rng.randrange(9 if depth < 3 else 7)
    if kind == 0:
        return jnull()
    if kind == 1:
        return jbool(rng.randrange(2) == 1)
    if kind == 2:
        return jint(rng.randrange(-2**31, 2**31))
    if kind == 3:
        return jdouble(rng.choice([1.0, -0.0, 0.5, 1e300, 1e-300, rng.random() * 1e6]))
    if kind == 4:
        return jstring(rng.choice(['', 'a', '<x>', 'é中', '\u2028', 'q"z', '\\', '\x01']))
    if kind == 5:
        return jlist([random_value(rng, depth + 1) for _ in range(rng.randrange(4))])
    if kind == 6:
        return jdict([(rng.choice(['a', 'b', '<k>', 'z']), random_value(rng, depth + 1))
                      for _ in range(rng.randrange(4))])
    return jstring('x' * rng.randrange(8))


def random_xml(rng):
    names = ['a', 'b', 'root', 'p:c', 'x-y']
    attrs = ['x="1"', 'y="&amp;"', 'xmlns:p="urn:x"', 'xmlns="urn:d"', 'id="a b"']
    body = []
    for _ in range(rng.randrange(1, 5)):
        kind = rng.randrange(7)
        if kind == 0:
            body.append('t' * rng.randrange(1, 5))
        elif kind == 1:
            body.append('&amp;&#65;')
        elif kind == 2:
            body.append('<!-- c -->')
        elif kind == 3:
            body.append('<![CDATA[x<y]]>')
        elif kind == 4:
            body.append('<?pi data?>')
        elif kind == 5:
            body.append('<' + rng.choice(names) + '/>')
        else:
            body.append('<' + rng.choice(names) + '>' + 'z' + '</' + rng.choice(names) + '>')
    name = rng.choice(names)
    attributes = ' '.join(rng.sample(attrs, rng.randrange(len(attrs) + 1)))
    return xml_request('<' + name + (' ' + attributes if attributes else '') + '>' + ''.join(body) + '</' + name + '>')


def cases(lane, seed):
    rng = random.Random(seed)
    result = []
    for index, text in enumerate(JSON_TEXTS):
        for options in (0, 1, 4, 16, 63) if lane == 'seen' else (2, 8, 32, 3, 5):
            request_text = text if lane == 'seen' else ' ' + text + ' '
            result.append((f'json-{index}-{options}', json_request(options, request_text)))
    for index, value in enumerate(WRITE_VALUES):
        for options in range(4):
            request_value = value if lane == 'seen' else jlist([value])
            result.append((f'write-{index}-{options}', write_request(options, request_value)))
    for index, text in enumerate(XML_TEXTS + XML_ERRORS):
        request_text = text if lane == 'seen' else '<!--h-->' + text
        result.append((f'xml-{index}', xml_request(request_text)))
    for index in range(80 if lane == 'seen' else 70):
        result.append((f'random-json-{index}', random_json(rng)))
        result.append((f'random-write-{index}', write_request(rng.randrange(4), random_value(rng))))
        result.append((f'random-xml-{index}', random_xml(rng)))
    return result
