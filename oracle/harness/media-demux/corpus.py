import math
import pathlib
import random
import struct
import subprocess

import flacgen

root = pathlib.Path(__file__).resolve().parents[3]
fixture_dir = root / '.work/tmp/media-demux/fixtures'

FIXTURES = [
    'chromeos/ash/resources/public/sounds/startup.wav',
    'chromeos/ash/resources/public/sounds/accessibility/camera_snap.wav',
    'chromeos/ash/resources/public/sounds/accessibility/lock.wav',
    'chromeos/ash/resources/public/sounds/accessibility/unlock.wav',
    'chromeos/ash/resources/public/sounds/accessibility/object_delete.wav',
    'chromeos/ash/resources/public/sounds/accessibility/shutdown.wav',
    'chromeos/ash/resources/public/sounds/accessibility/spoken_feedback_enabled.wav',
    'chromeos/ash/resources/public/sounds/accessibility/spoken_feedback_disabled.wav',
    'chromeos/ash/resources/public/sounds/accessibility/spoken_feedback_toggle_countdown_high.wav',
    'chromeos/ash/resources/public/sounds/accessibility/spoken_feedback_toggle_countdown_low.wav',
    'chromeos/ash/resources/public/sounds/accessibility/touch_type.wav',
    'chromeos/ash/resources/public/sounds/accessibility/volume_adjust.wav',
    'chromeos/ash/resources/public/sounds/accessibility/earcons/passthrough.wav',
    'chromeos/ash/resources/public/sounds/accessibility/earcons/exit_screen.wav',
    'chromeos/ash/resources/public/sounds/accessibility/earcons/enter_screen.wav',
    'third_party/webrtc/data/voice_engine/audio_tiny8.wav',
    'third_party/webrtc/data/voice_engine/audio_tiny11.wav',
    'third_party/webrtc/data/voice_engine/audio_tiny16.wav',
    'third_party/webrtc/data/voice_engine/audio_tiny22.wav',
    'third_party/webrtc/data/voice_engine/audio_tiny32.wav',
    'third_party/webrtc/data/voice_engine/audio_tiny44.wav',
    'third_party/webrtc/data/voice_engine/audio_long16.wav',
    'third_party/webrtc/data/voice_engine/audio_long8mulaw.wav',
    'third_party/google_input_tools/src/chrome/os/sounds/KeypressDelete.wav',
    'third_party/google_input_tools/src/chrome/os/sounds/KeypressStandard.wav',
    'third_party/google_input_tools/src/chrome/os/sounds/KeypressReturn.wav',
    'third_party/google_input_tools/src/chrome/os/sounds/KeypressSpacebar.wav',
    'third_party/google_input_tools/src/chrome/os/sounds/audio_end.wav',
    'third_party/google_input_tools/src/chrome/os/sounds/audio_initiate.wav',
    'third_party/google_input_tools/src/chrome/os/sounds/auto_correction.wav',
    'media/base/android/java/res/raw/empty.wav',
    'third_party/blink/renderer/platform/audio/resources/IRC_Composite_C_R0195_T000_P000.wav',
    'third_party/blink/renderer/platform/audio/resources/IRC_Composite_C_R0195_T015_P030.wav',
    'third_party/blink/renderer/platform/audio/resources/IRC_Composite_C_R0195_T030_P060.wav',
    'third_party/blink/renderer/platform/audio/resources/IRC_Composite_C_R0195_T045_P090.wav',
    'third_party/blink/renderer/platform/audio/resources/IRC_Composite_C_R0195_T060_P315.wav',
    'third_party/blink/renderer/platform/audio/resources/IRC_Composite_C_R0195_T075_P345.wav',
    'third_party/blink/renderer/platform/audio/resources/Composite.flac',
    'chromeos/ash/resources/public/sounds/power/low_battery.flac',
    'chromeos/ash/resources/public/sounds/power/charging_started_low.flac',
    'chrome/browser/resources/intro/sounds/logo.flac',
    'chrome/browser/resources/intro/sounds/all_set.flac',
    'components/neterror/resources/sounds/hit.mp3',
    'components/neterror/resources/sounds/button-press.mp3',
    'components/neterror/resources/sounds/score-reached.mp3',
    'chromeos/ash/resources/public/sounds/focus_mode_ending_moment.flac',
    'chromeos/ash/resources/public/sounds/power/charging_started_medium.flac',
    'chromeos/ash/resources/public/sounds/power/charging_started_high.flac',
    'chrome/browser/resources/intro/sounds/feature_showcase_progress.flac',
    'chrome/browser/resources/intro/sounds/welcome_back.flac',
    'chrome/browser/resources/intro/sounds/feature_showcase_ambient.flac',
    'chrome/browser/resources/intro/sounds/ambient.flac',
]


def fixtures():
    if not all((fixture_dir / path).exists() for path in FIXTURES):
        fixture_dir.mkdir(parents=True, exist_ok=True)
        image = subprocess.run(['sh', 'oracle/source-oracle', 'image-tag'], cwd=root, check=True, text=True, stdout=subprocess.PIPE).stdout.strip()
        archive = subprocess.run(['docker', 'run', '--rm', '--network', 'none', '--mount', 'type=volume,src=manganese-oracle-work-amd64,dst=/work,readonly', image,
                                  'tar', 'cf', '-', '-C', '/work/src', *FIXTURES], check=True, stdout=subprocess.PIPE).stdout
        subprocess.run(['tar', 'xf', '-', '-C', str(fixture_dir)], input=archive, check=True)
    return [(path, (fixture_dir / path).read_bytes()) for path in FIXTURES]


def samples(rng, frames, channels, bits, kind='sine'):
    width = (bits + 7) // 8
    out = bytearray()
    phase = rng.random() * 6.28
    step = 0.01 + rng.random() * 0.2
    for i in range(frames):
        for c in range(channels):
            value = math.sin(phase + i * step * (c + 1)) * 0.4 + (rng.random() - 0.5) * 0.02
            if kind == 'float':
                out += struct.pack('<f', value) if width == 4 else struct.pack('<d', value)
            elif width == 1:
                out.append(int(128 + value * 100) & 255)
            else:
                scaled = int(value * (1 << (8 * width - 2)))
                out += scaled.to_bytes(width, 'little', signed=True)
    return bytes(out)


def chunk(tag, body, pad=True):
    data = tag + struct.pack('<I', len(body)) + body
    if pad and len(body) & 1:
        data += b'\0'
    return data


def fmt_body(tag=1, channels=2, rate=44100, bits=16, block_align=None, byte_rate=None, size=16, cb_extra=b'', extensible=None, cb_size=None):
    if block_align is None:
        block_align = channels * ((bits + 7) // 8)
    if byte_rate is None:
        byte_rate = rate * block_align
    body = struct.pack('<HHIIH', tag & 0xffff, channels & 0xffff, rate & 0xffffffff, byte_rate & 0xffffffff, block_align & 0xffff)
    if size == 14:
        return body
    body += struct.pack('<H', bits & 0xffff)
    if size == 16:
        return body
    extra = cb_extra
    if extensible is not None:
        valid_bits, mask, guid = extensible
        extra = struct.pack('<HI', valid_bits, mask) + guid + cb_extra
    body += struct.pack('<H', len(extra) if cb_size is None else cb_size) + extra
    return body


def guid(tag, base=bytes([0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71])):
    return struct.pack('<I', tag) + base


AMBISONIC = bytes([0x21, 0x07, 0xD3, 0x11, 0x86, 0x44, 0xC8, 0xC1, 0xCA, 0x00, 0x00, 0x00])
BROKEN = bytes([0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA])


def wav(fmt, data, before=(), after=(), riff=b'RIFF', riff_size=None, data_size=None, fmt_first=True, pad=True):
    data_chunk = b'data' + struct.pack('<I', len(data) if data_size is None else data_size & 0xffffffff) + data
    if pad and len(data) & 1:
        data_chunk += b'\0'
    chunks = list(before)
    if fmt is not None:
        chunks.insert(0 if fmt_first else len(chunks), chunk(b'fmt ', fmt))
    body = b'WAVE' + b''.join(chunks) + data_chunk + b''.join(after)
    return riff + struct.pack('<I', (len(body) if riff_size is None else riff_size) & 0xffffffff) + body


def rf64(fmt, data, kind=b'RF64', data_size=None, sample_count=0):
    ds64 = struct.pack('<QQQ', 0, len(data) if data_size is None else data_size, sample_count)
    body = b'WAVE' + chunk(b'ds64', ds64) + chunk(b'fmt ', fmt) + b'data' + struct.pack('<I', 0xffffffff) + data
    return kind + struct.pack('<I', 0xffffffff) + body


def id3(version=3, flags=0, size=20, frames=b''):
    body = frames.ljust(size, b'\0')
    syncsafe = bytes([(len(body) >> 21) & 0x7f, (len(body) >> 14) & 0x7f, (len(body) >> 7) & 0x7f, len(body) & 0x7f])
    tag = b'ID3' + bytes([version, 0, flags]) + syncsafe + body
    if version == 4 and flags & 0x10:
        tag += b'3DI' + bytes([version, 0, flags]) + syncsafe
    return tag


def text_frame(name, text):
    payload = b'\x00' + text
    return name + struct.pack('>I', len(payload)) + b'\0\0' + payload


def mpeg_frames(count, header=0xFFFB9064):
    frame = struct.pack('>I', header) + bytes(413)
    return frame * count


def adts_frames(count):
    frame = bytearray(b'\xff\xf1\x50\x80\x00\x1f\xfc') + bytes(100)
    length = len(frame)
    frame[3] = (frame[3] & 0xfc) | (length >> 11)
    frame[4] = (length >> 3) & 0xff
    frame[5] = ((length & 7) << 5) | 0x1f
    return bytes(frame) * count


def mutate(data, rng, operations=None):
    data = bytearray(data)
    for _ in range(operations or rng.randrange(1, 5)):
        op = rng.randrange(8)
        if not data:
            data.append(rng.randrange(256))
            continue
        position = rng.randrange(len(data))
        if op == 0:
            data[position] ^= 1 << rng.randrange(8)
        elif op == 1:
            data[position] = rng.choice([0, 1, 0x7f, 0x80, 0xff, rng.randrange(256)])
        elif op == 2:
            data[position:position] = bytes(rng.randrange(256) for _ in range(rng.randrange(1, 9)))
        elif op == 3:
            del data[position:position + rng.randrange(1, 65)]
        elif op == 4:
            del data[position:]
        elif op == 5 and len(data) >= 4:
            position = rng.randrange(len(data) - 3)
            value = rng.choice([0, 1, 2, 3, 4, 0xffffffff, 0x7fffffff, 0x80000000, len(data), len(data) - position, rng.randrange(1 << 32)])
            data[position:position + 4] = struct.pack('<I', value & 0xffffffff)
        elif op == 6:
            end = min(len(data), position + rng.randrange(1, 257))
            data[end:end] = data[position:end]
        else:
            data[position] = (data[position] + rng.choice([-1, 1])) & 0xff
    return bytes(data)


WAV_TAGS = [(0x0001, 'pcm_s16le'), (0x0001, 'pcm_u8'), (0x0001, 'pcm_s24le'), (0x0001, 'pcm_s32le'), (0x0001, 'pcm_s64le'), (0x0002, 'adpcm_ms'), (0x0003, 'pcm_f32le'), (0x0003, 'pcm_f64le'), (0x0006, 'pcm_alaw'), (0x0007, 'pcm_mulaw'), (0x000A, 'wmavoice'), (0x0010, 'adpcm_ima_oki'), (0x0011, 'adpcm_ima_wav'), (0x0011, 'adpcm_zork'), (0x0017, 'adpcm_ima_oki'), (0x0020, 'adpcm_yamaha'), (0x0022, 'truespeech'), (0x0031, 'gsm_ms'), (0x0032, 'gsm_ms'), (0x0038, 'amr_nb'), (0x0042, 'g723_1'), (0x0045, 'adpcm_g726'), (0x0014, 'adpcm_g726'), (0x0040, 'adpcm_g726'), (0x0041, 'g728'), (0x0050, 'mp2'), (0x0055, 'mp3'), (0x0057, 'amr_nb'), (0x0058, 'amr_wb'), (0x0061, 'adpcm_ima_dk4'), (0x0062, 'adpcm_ima_dk3'), (0x0064, 'adpcm_g726'), (0x0069, 'adpcm_ima_xbox'), (0x0075, 'metasound'), (0x0083, 'g729'), (0x00ff, 'aac'), (0x0111, 'g723_1'), (0x0125, 'adpcm_sanyo'), (0x0130, 'sipr'), (0x0135, 'acelp.kelvin'), (0x0160, 'wmav1'), (0x0161, 'wmav2'), (0x0162, 'wmapro'), (0x0163, 'wmalossless'), (0x0165, 'xma1'), (0x0166, 'xma2'), (0x0180, 'ftr'), (0x0200, 'adpcm_ct'), (0x0215, 'dvaudio'), (0x0216, 'dvaudio'), (0x0270, 'atrac3'), (0x028E, 'msnsiren'), (0x028F, 'adpcm_g722'), (0x0350, 'misc4'), (0x0401, 'imc'), (0x0402, 'iac'), (0x0500, 'on2avc'), (0x0501, 'on2avc'), (0x1500, 'gsm_ms'), (0x1501, 'truespeech'), (0x1600, 'aac'), (0x1602, 'aac_latm'), (0x1610, 'aac'), (0x2000, 'ac3'), (0x2000, 'eac3'), (0x2001, 'dts'), (0x2048, 'sonic'), (0x2048, 'sonicls'), (0x2222, 'g729'), (0x6c75, 'pcm_mulaw'), (0x706d, 'aac'), (0x4143, 'aac'), (0x4180, 'ftr'), (0x594a, 'xan_dpcm'), (0x729A, 'g729'), (0x8180, 'ftr'), (0xA100, 'g723_1'), (0xA106, 'aac'), (0xA109, 'speex'), (0xCD02, 'g728'), (0xF1AC, 'flac'), (0xFFFE, 'dfpwm')]
PARSED_CODECS = {'mp1', 'mp2', 'mp3', 'aac', 'flac', 'opus', 'vorbis'}
MPEG2_GUID = bytes([0x2B, 0x80, 0x6D, 0xE0, 0x46, 0xDB, 0xCF, 0x11, 0xB4, 0xD1, 0x00, 0x80, 0x5F, 0x6C, 0xBB, 0xEA])


def upper4(tag):
    return int.from_bytes(bytes(b - 32 if 97 <= b <= 122 else b for b in tag.to_bytes(4, 'little')), 'little')


def wav_codec(tag):
    for known, name in WAV_TAGS:
        if known == tag:
            return name
    for known, name in WAV_TAGS:
        if upper4(known) == upper4(tag):
            return name
    return None


def carries_parsed_codec(data):
    start = 0
    while True:
        at = data.find(b'fmt ', start)
        if at < 0:
            return False
        start = at + 1
        body = data[at + 8:at + 48]
        if len(body) < 2:
            continue
        for order in ('<', '>'):
            tag = struct.unpack(order + 'H', body[:2])[0]
            if wav_codec(tag) in PARSED_CODECS:
                return True
            if tag == 0xFFFE and len(body) >= 40 and (body[24:40] == MPEG2_GUID or wav_codec(struct.unpack('<I', body[24:28])[0]) in PARSED_CODECS):
                return True


def generated(lane):
    seen = lane == 'seen'
    rng = random.Random(4417 if seen else 9931)
    items = []

    def add(name, data):
        items.append((name, data))

    rates = [8000, 16000, 22050, 44100, 48000, 96000] if seen else [11025, 24000, 32000, 88200, 192000]
    for bits in ([8, 16, 24, 32] if seen else [8, 16, 24]):
        for channels in ([1, 2, 6] if seen else [2, 4, 8]):
            for rate in rates[:3] if bits != 16 else rates:
                frames = rng.choice([0, 1, 17, 1023, 1024, 1025, 3000, 20000])
                add(f'pcm-{bits}-{channels}-{rate}-{frames}', wav(fmt_body(1, channels, rate, bits), samples(rng, frames, channels, bits)))
    for bits in ([12, 20, 4, 40, 64, 0, 33] if seen else [10, 18, 2, 48, 56, 9]):
        add(f'pcm-odd-bits-{bits}', wav(fmt_body(1, 2, 44100, bits), samples(rng, 500, 2, max(bits, 8))))
    for tag, bits, name in ([(3, 32, 'f32'), (3, 64, 'f64'), (3, 16, 'f16'), (6, 8, 'alaw'), (7, 8, 'mulaw'), (0x6c75, 8, 'ul'), (0x4c55, 8, 'ul-upper'),
                             (2, 4, 'adpcm-ms'), (0x11, 4, 'ima'), (0x1234, 16, 'unknown'), (0, 16, 'zero'), (0x31, 0, 'gsm'), (0x2000, 16, 'ac3'),
                             (0x1602, 16, 'latm'), (0xFFFE, 16, 'extensible-short'), (0x0165, 16, 'xma1')] if seen else
                            [(3, 32, 'f32-mono'), (6, 8, 'alaw-stereo'), (7, 8, 'mulaw-stereo'), (0x6C55, 8, 'ul-mixed'), (0x45, 4, 'g726'),
                             (0x4321, 8, 'unknown'), (0x22, 16, 'truespeech'), (0x2001, 16, 'dts'), (0x0166, 16, 'xma2-tag')]):
        channels = 1 if name.endswith('mono') else 2
        kind = 'float' if tag == 3 and bits in (32, 64) else 'sine'
        add(f'codec-{name}', wav(fmt_body(tag, channels, 22050, bits), samples(rng, 2000, channels, max(bits, 8), kind)))
    masks = [(1, 0x4), (2, 0x3), (4, 0x33), (6, 0x60F), (6, 0x3F), (8, 0x63F), (12, 0x2D63F), (6, 0x3), (2, 0), (3, 0x7), (2, 0x60000000)] if seen else \
        [(1, 0x4), (2, 0x3), (3, 0x103), (5, 0x607), (6, 0x70F), (7, 0x13F), (8, 0xFF), (10, 0x2D03F), (4, 0xF), (2, 0x1)]
    for channels, mask in masks:
        add(f'extensible-{channels}-{mask:x}', wav(fmt_body(0xFFFE, channels, 48000, 16, size=40, extensible=(16, mask, guid(1))), samples(rng, 1500, channels, 16)))
    for name, sub, bits in ([('float', guid(3), 32), ('alaw', guid(6), 8), ('ambisonic', guid(1, AMBISONIC), 16), ('broken', guid(1, BROKEN), 16),
                             ('unknown-guid', bytes(range(16)), 16), ('s24', guid(1), 24)] if seen else
                            [('mulaw', guid(7), 8), ('pcm32', guid(1), 32), ('ambisonic-float', guid(3, AMBISONIC), 32), ('zero-guid', bytes(16), 16)]):
        kind = 'float' if bits == 32 and name.endswith('float') else 'sine'
        add(f'extensible-{name}', wav(fmt_body(0xFFFE, 2, 44100, bits, size=40, extensible=(bits, 0x3, sub)), samples(rng, 1200, 2, bits, kind)))
    base = samples(rng, 4000, 2, 16)
    fmt = fmt_body(1, 2, 44100, 16)
    info = chunk(b'LIST', b'INFO' + chunk(b'INAM', b'Corpus title\0') + chunk(b'IART', b'Generated\0'))
    layouts = {
        'list-before-data': wav(fmt, base, before=[info]),
        'list-after-data': wav(fmt, base, after=[info]),
        'fact-right': wav(fmt, base, before=[chunk(b'fact', struct.pack('<I', 4000))]),
        'fact-wrong': wav(fmt, base, before=[chunk(b'fact', struct.pack('<I', 123))]),
        'fact-large': wav(fmt, base, before=[chunk(b'fact', struct.pack('<I', 10 ** 9))]),
        'junk-odd': wav(fmt, base, before=[chunk(b'junk', b'abc'), chunk(b'JUNK', bytes(31))]),
        'junk-unpadded': wav(fmt, base, before=[chunk(b'junk', b'abc', pad=False)]),
        'data-before-fmt': wav(fmt, base, before=[chunk(b'junk', b'x' * 8)], fmt_first=False),
        'two-fmt': wav(fmt, base, before=[chunk(b'fmt ', fmt_body(1, 1, 8000, 8))]),
        'no-data': b'RIFF' + struct.pack('<I', 28) + b'WAVE' + chunk(b'fmt ', fmt),
        'no-fmt': wav(None, base),
        'data-size-zero': wav(fmt, base, data_size=0),
        'data-size-max': wav(fmt, base, data_size=0xffffffff),
        'data-size-large': wav(fmt, base, data_size=len(base) * 3),
        'data-size-small': wav(fmt, base, data_size=len(base) // 3),
        'riff-size-wrong': wav(fmt, base, riff_size=7),
        'odd-data': wav(fmt_body(1, 1, 8000, 8), samples(rng, 333, 1, 8)),
        'trailing-data-chunk': wav(fmt, base[:1000], after=[chunk(b'data', base[1000:3000])]),
        'cue': wav(fmt, base, before=[chunk(b'cue ', struct.pack('<I', 2) + bytes(48))]),
        'cue-short': wav(fmt, base, before=[chunk(b'cue ', struct.pack('<I', 9) + bytes(24))]),
        'bext': wav(fmt, base, before=[chunk(b'bext', b'desc'.ljust(256, b'\0') + bytes(346))]),
        'bext-history': wav(fmt, base, before=[chunk(b'bext', bytes(602) + b'A=PCM\r\n')]),
        'id3-chunk': wav(fmt, base, after=[chunk(b'id3 ', id3(frames=text_frame(b'TIT2', b'chunk')))]),
        'smv-bad-version': wav(fmt, base, before=[chunk(b'SMV0', b'')]),
        'list-short': wav(fmt, base, before=[chunk(b'LIST', b'ab')]),
        'rifx': b'RIFX' + struct.pack('>I', 36 + 2000) + b'WAVE' + b'fmt ' + struct.pack('>I', 16) +
                struct.pack('>HHIIHH', 1, 2, 44100, 176400, 4, 16) + b'data' + struct.pack('>I', 2000) + base[:2000],
        'rifx-extensible': b'RIFX' + struct.pack('>I', 38 + 2000) + b'WAVE' + b'fmt ' + struct.pack('>I', 18) +
                           struct.pack('>HHIIHHH', 1, 2, 44100, 176400, 4, 16, 0) + b'data' + struct.pack('>I', 2000) + base[:2000],
        'rf64': rf64(fmt, base),
        'bw64': rf64(fmt, base, kind=b'BW64'),
        'rf64-samples': rf64(fmt, base, sample_count=4000),
        'rf64-negative': rf64(fmt, base, data_size=1 << 63),
        'fmt-14': wav(fmt_body(1, 1, 8000, 8, size=14), samples(rng, 800, 1, 8)),
        'fmt-18': wav(fmt_body(1, 2, 44100, 16, size=18), base),
        'fmt-20-extra': wav(fmt_body(1, 2, 44100, 16, size=18, cb_extra=b'\x01\x00'), base),
        'fmt-cb-overflow': wav(fmt_body(1, 2, 44100, 16, size=18, cb_extra=b'\x05\x06', cb_size=200), base),
        'fmt-short': wav(fmt_body(1, 2, 44100, 16)[:12], base),
        'rate-zero': wav(fmt_body(1, 2, 0, 16), base),
        'rate-low': wav(fmt_body(1, 2, 2000, 16), base),
        'rate-high': wav(fmt_body(1, 2, 800000, 16), base),
        'rate-negative': wav(fmt_body(1, 2, 0x80000000, 16), base),
        'channels-zero': wav(fmt_body(1, 0, 44100, 16, block_align=4), base),
        'channels-33': wav(fmt_body(1, 33, 44100, 16), base),
        'channels-600': wav(fmt_body(1, 600, 44100, 16), base),
        'block-align-zero': wav(fmt_body(1, 2, 44100, 16, block_align=0), base),
        'block-align-one': wav(fmt_body(1, 2, 44100, 16, block_align=1), base),
        'block-align-odd': wav(fmt_body(1, 2, 44100, 16, block_align=5), base),
        'block-align-huge': wav(fmt_body(1, 2, 44100, 16, block_align=9000), base * 3),
        'byte-rate-wrong': wav(fmt_body(1, 2, 44100, 16, byte_rate=1), base),
        'f24-in-s24': wav(fmt_body(1, 2, 44100, 24, block_align=8), base),
        'f16-in-s32': wav(fmt_body(1, 2, 44100, 32, size=18, cb_extra=b'\x01\x00'), base),
        'id3-prefix': id3(frames=text_frame(b'TIT2', b'prefix')) + wav(fmt, base),
        'id3-prefix-odd': id3(size=21, frames=text_frame(b'TIT2', b'odd')) + wav(fmt, base),
        'id3-v4-footer': id3(version=4, flags=0x10, frames=text_frame(b'TIT2', b'footer')) + wav(fmt, base),
        'id3-v2': id3(version=2, size=16) + wav(fmt, base),
        'id3-twice': id3() + id3(size=11) + wav(fmt, base),
        'id3-extended': id3(version=3, flags=0x40, frames=bytes([0, 0, 0, 6, 0, 0, 0, 0, 0, 0]) + text_frame(b'TIT2', b'ext')) + wav(fmt, base),
    }
    keep = sorted(layouts) if seen else sorted(layouts)[1::2]
    for name in keep:
        data = layouts[name]
        if not seen:
            data = mutate(data, rng, 1) if name.startswith(('fact', 'junk', 'list')) else data
        add(f'layout-{name}', data)
    long = samples(rng, 140000 if seen else 90000, 1, 16)
    add('long-mono', wav(fmt_body(1, 1, 22050 if seen else 16000, 16), long))
    long_stereo = samples(rng, 60000, 2, 24)
    add('long-stereo-24', wav(fmt_body(1, 2, 96000, 24), long_stereo))
    full = wav(fmt_body(1, 2, 32000, 16), samples(rng, 3000, 2, 16))
    cuts = [0, 1, 4, 8, 11, 12, 16, 20, 24, 36, 40, 43, 44, 45, 46, 100, 1001, len(full) // 2, len(full) - 1] if seen else \
        [2, 6, 13, 22, 30, 39, 42, 47, 77, 2001, len(full) - 3]
    for cut in cuts:
        add(f'truncated-{cut}', full[:cut])
    others = {
        'empty': b'',
        'one-byte': b'R',
        'text': b'Plain text, not media at all.\n' * 20,
        'random': bytes(rng.randrange(256) for _ in range(3000)),
        'zeros': bytes(5000),
        'hls': b'#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nsegment.ts\n',
        'hls-master': b'#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nlow.m3u8\n',
        'm3u-plain': b'#EXTM3U\n#EXTINF:123,Track\ntrack.mp3\n',
        'ogg-magic': b'OggS\0\x02' + bytes(100),
        'flac-magic': b'fLaC\0\0\0\x22' + bytes(34),
        'ebml-magic': bytes([0x1A, 0x45, 0xDF, 0xA3, 0x84]) + b'webm' + bytes(100),
        'ftyp': struct.pack('>I', 16) + b'ftypisom' + bytes(4) + struct.pack('>I', 8) + b'mdat',
        'mpeg-frames': mpeg_frames(10),
        'mpeg-few': mpeg_frames(2),
        'adts': adts_frames(8),
        'id3-only': id3(size=200),
        'id3-mpeg': id3() + mpeg_frames(8),
        'riff-avi': b'RIFF' + struct.pack('<I', 100) + b'AVI LIST' + bytes(100),
    } if seen else {
        'single-zero': b'\0',
        'text-lines': b'line\r\n' * 300,
        'random-long': bytes(rng.randrange(256) for _ in range(9000)),
        'hls-sequence': b'#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:4\n',
        'ogg-bad-version': b'OggS\0\x09' + bytes(60),
        'mpeg-layer2': mpeg_frames(9, 0xFFFD9004),
        'adts-few': adts_frames(2),
        'wave-no-riff': b'XXXX' + struct.pack('<I', 100) + b'WAVE' + bytes(100),
    }
    for name, data in others.items():
        add(f'other-{name}', data)
    bases = [wav(fmt_body(1, 2, 44100, 16), samples(rng, 2500, 2, 16)), wav(fmt_body(1, 1, 8000, 8), samples(rng, 3000, 1, 8)),
             wav(fmt_body(0xFFFE, 6, 48000, 24, size=40, extensible=(24, 0x60F, guid(1))), samples(rng, 900, 6, 24)),
             wav(fmt_body(3, 2, 48000, 32), samples(rng, 1500, 2, 32, 'float')), wav(fmt_body(7, 1, 8000, 8), samples(rng, 4000, 1, 8))]
    count = 0
    while count < (60 if seen else 40):
        data = mutate(rng.choice(bases), rng)
        if carries_parsed_codec(data):
            continue
        add(f'mutated-{count}', data)
        count += 1
    return items


def flac_generated(lane):
    seen = lane == 'seen'
    rng = random.Random(7121 if seen else 3389)
    items = []
    g = flacgen

    def add(name, data):
        items.append((f'flac-{name}', data))

    def simple(channels=2, bps=16, rate=44100, blocksize=4096, frames=12, metadata=(), info=None, **options):
        out, total, blocks = g.stream(rng, channels, bps, rate, blocksize, frames, **options)
        return g.flac_file(out, rate, channels, bps, total, blocks, metadata, info), out, total, blocks

    grid = [(1, 8, 8000, 576, 4), (1, 16, 22050, 1152, 11), (2, 16, 44100, 4096, 12), (2, 24, 48000, 4608, 10), (6, 16, 48000, 1024, 15),
            (8, 24, 96000, 2048, 9), (2, 16, 16000, 192, 40), (4, 16, 32000, 256, 20), (3, 12, 24000, 512, 13), (5, 20, 88200, 8192, 3),
            (7, 16, 44100, 2304, 10), (2, 32, 192000, 4096, 10), (1, 24, 176400, 16384, 2), (2, 8, 11025, 100, 30), (2, 16, 50000, 1000, 25)] if seen else \
           [(1, 16, 44100, 4096, 10), (2, 16, 48000, 1152, 14), (2, 24, 96000, 4096, 11), (6, 24, 48000, 4096, 10), (4, 8, 8000, 2304, 9),
            (8, 16, 44100, 576, 12), (2, 20, 32000, 300, 33), (1, 12, 37800, 2000, 16), (3, 16, 22050, 8192, 5), (2, 16, 64000, 1500, 18)]
    for channels, bps, rate, blocksize, frames in grid:
        add(f'{channels}ch-{bps}-{rate}-{blocksize}-{frames}', simple(channels, bps, rate, blocksize, frames)[0])
    for mode, name in ([(8, 'left-side'), (9, 'right-side'), (10, 'mid-side')] if seen else [(10, 'mid-side'), (8, 'left-side')]):
        add(f'stereo-{name}', simple(2, 16, 44100, 4096, 12, mode=mode)[0])
    for kind in (['verbatim', 'constant', 'fixed2'] if seen else ['verbatim', 'fixed2']):
        add(f'subframe-{kind}', simple(2, 16, 44100, 1152, 12, kind=kind)[0])
    add('noise', simple(2, 16, 44100, 4096, 11, shape='noise', kind='verbatim')[0])
    add('silence', simple(1 if seen else 2, 16, 8000, 4096, 12 if seen else 13, shape='silence')[0])
    add('variable', simple(2, 16, 44100, 0, 0, variable=True, sizes=[4096, 1024, 2048, 4096, 576, 4608, 4096, 192, 4096, 1152, 4096, 4096, 300])[0])
    add('variable-short', simple(1, 16, 22050, 0, 0, variable=True, sizes=[1024, 2048, 4096])[0])
    add('blocksize-change', simple(2, 16, 44100, 0, 0, sizes=[4096] * 11 + [1024])[0])
    for frames in ([1, 2, 5, 9, 10, 11] if seen else [3, 8, 12]):
        add(f'frames-{frames}', simple(2, 16, 44100, 4096, frames)[0])
    for name, options in ([('rate-code-12', dict(rate=32000, rate_code=12)), ('rate-code-13', dict(rate=11025, rate_code=13)),
                           ('rate-code-14', dict(rate=50000, rate_code=14)), ('bps-code-0', dict(bps_code=0))] if seen else
                          [('rate-code-13', dict(rate=44100, rate_code=13)), ('rate-code-12', dict(rate=48000, rate_code=12)), ('bps-code-0', dict(bps_code=0))]):
        rate = options.pop('rate', 44100)
        add(name, simple(2, 16, rate, 4096, 12, **options)[0])
    add('rate-code-0-single', simple(1, 16, 44100, 4096, 1, rate_code=0)[0])
    add('rate-code-0-first', simple(1, 16, 44100, 4096, 12, rate_code=None)[0])
    base, frames, total, blocks = simple(2, 16, 44100, 4096, 12)
    sizes = [len(f) for f in frames]
    info = lambda **change: g.streaminfo(change.get('min_block', 4096), change.get('max_block', 4096), min(sizes), max(sizes), change.get('rate', 44100),
                                         change.get('channels', 2), change.get('bps', 16), change.get('samples', total))
    infos = {
        'samples-zero': info(samples=0), 'samples-large': info(samples=total * 3), 'samples-small': info(samples=total // 4),
        'rate-zero': info(rate=0), 'rate-mismatch': info(rate=22050), 'rate-huge': info(rate=1000000), 'rate-low': info(rate=2000),
        'channels-mismatch': info(channels=6), 'channels-fewer': info(channels=1), 'bps-mismatch': info(bps=24), 'bps-invalid': info(bps=3),
        'bps-eight': info(bps=8), 'max-block-small': info(max_block=8, min_block=8), 'max-block-under-frame': info(max_block=1024, min_block=1024),
        'min-block-small': info(min_block=8), 'min-over-max': info(min_block=8192),
    }
    for name in sorted(infos) if seen else sorted(infos)[::3]:
        add(f'info-{name}', g.flac_file(frames, 44100, 2, 16, total, blocks, info=infos[name]))
    comment = lambda *pairs: (4, g.vorbis_comment([p for p in pairs]))
    metadata = {
        'padding': [(1, bytes(1024))],
        'application': [(2, b'test' + bytes(20))],
        'seektable': [(3, g.seektable([(0, 0, 4096), (40960, sum(sizes[:10]), 4096)]))],
        'seektable-odd': [(3, g.seektable([(0, 0, 4096)]) + b'abc')],
        'comment': [comment(b'TITLE=Corpus', b'ARTIST=Manganese')],
        'mask-stereo': [comment(b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x3')],
        'mask-decimal': [comment(b'waveformatextensible_channel_mask=3')],
        'mask-wide': [comment(b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x603')],
        'mask-side': [comment(b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x600')],
        'mask-zero': [comment(b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0')],
        'mask-high': [comment(b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x40000')],
        'mask-negative': [comment(b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=-3')],
        'mask-octal': [comment(b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=03')],
        'mask-text': [comment(b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=front')],
        'mask-twice': [comment(b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x3', b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x4')],
        'mask-blocks': [comment(b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=x'), comment(b'WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x3')],
        'comment-short': [(4, b'\x05\0\0\0ab')],
        'comment-truncated': [(4, g.vorbis_comment([b'TITLE=x'])[:-3])],
        'cuesheet': [(5, g.cuesheet())],
        'cuesheet-short': [(5, g.cuesheet()[:400])],
        'cuesheet-no-index': [(5, g.cuesheet(index_points=0))],
        'cuesheet-no-tracks': [(5, g.cuesheet(tracks=0) + bytes(40))],
        'picture-png': [(6, g.picture())],
        'picture-jpeg': [(6, g.picture(b'image/jpeg', b'\xff\xd8\xff\xe0' + bytes(60)))],
        'picture-png-sig-jpeg-mime': [(6, g.picture(b'image/jpeg'))],
        'picture-unknown-mime': [(6, g.picture(b'image/x-unknown'))],
        'picture-type-invalid': [(6, g.picture(kind=30))],
        'picture-width-zero': [(6, g.picture(width=0))],
        'picture-short': [(6, g.picture()[:30])],
        'picture-mime-long': [(6, g.picture(b'image/' + b'x' * 70))],
        'picture-length-large': [(6, g.picture(length=1000))],
        'picture-two': [(6, g.picture()), (6, g.picture(b'image/gif', b'GIF89a' + bytes(30), kind=4))],
        'block-reserved': [(9, bytes(12))],
        'block-invalid': [(127, bytes(4))],
    }
    for name in sorted(metadata) if seen else sorted(metadata)[1::3]:
        add(f'meta-{name}', g.flac_file(frames, 44100, 2, 16, total, blocks, metadata[name]))
    add('meta-picture-truncated', g.flac_file(frames, 44100, 2, 16, total, blocks, [(6, g.picture(data=b'\x89PNG\r\n\x1a\n' + bytes(56), length=(1 << 24) + 64))]))
    head = b'fLaC' + g.block(0, info())
    joined = b''.join(frames)
    layouts = {
        'no-marker': joined,
        'no-marker-at-3': joined[sum(sizes[:3]):],
        'marker-only': b'fLaC' if seen else b'fLaC\x80\0',
        'info-only': head,
        'no-last-flag': b'fLaC' + bytes([0]) + (34).to_bytes(3, 'big') + info() + joined,
        'comment-first': b'fLaC' + g.block(4, g.vorbis_comment([b'A=b'])) + g.block(0, info(), True) + joined,
        'padding-first': b'fLaC' + g.block(1, bytes(10)) + g.block(0, info(), True) + joined,
        'seektable-first': b'fLaC' + g.block(3, g.seektable([(0, 0, 4096)]), True) + joined,
        'two-streaminfo': b'fLaC' + g.block(0, info()) + g.block(0, info(), True) + joined,
        'streaminfo-size': b'fLaC' + g.block(0, info() + bytes(2), True) + joined,
        'block-beyond-end': b'fLaC' + g.block(0, info()) + bytes([0x81]) + (100000).to_bytes(3, 'big') + bytes(10),
        'junk-before-frames': base[:len(head)] + bytes(range(256)) * 3 + joined,
        'zeros-before-frames': base[:len(head)] + bytes(5000) + joined,
        'junk-between': base[:len(head)] + b''.join(f + (bytes(range(7, 250)) if i == 4 else b'') for i, f in enumerate(frames)),
        'frame-crc-bad': simple(2, 16, 44100, 4096, 12, corrupt=(3,))[0],
        'frame-crc-bad-first': simple(2, 16, 44100, 4096, 12, corrupt=(0,))[0],
        'frame-crc-bad-many': simple(2, 16, 44100, 4096, 14, corrupt=(1, 2, 5, 6, 7))[0],
        'frame-numbers-gap': simple(2, 16, 44100, 4096, 12, numbers=[0, 1, 2, 3, 10, 11, 12, 13, 14, 15, 16, 17])[0],
        'frame-numbers-start': simple(2, 16, 44100, 4096, 12, numbers=list(range(100, 112)))[0],
        'frame-numbers-repeat': simple(2, 16, 44100, 4096, 12, numbers=[0, 1, 2, 2, 3, 4, 5, 6, 7, 8, 9, 10])[0],
        'frames-reordered': base[:len(head)] + b''.join(frames[i] for i in [0, 1, 3, 2, 4, 5, 7, 6, 8, 9, 10, 11]),
        'frames-duplicated': base[:len(head)] + b''.join(frames[:6] + frames[3:]),
        'trailing-junk': base + bytes(range(256)) * 4,
        'trailing-id3v1': base + b'TAG' + bytes(125),
        'id3-prefix': id3(frames=text_frame(b'TIT2', b'flac')) + base,
        'id3-prefix-twice': id3() + id3(size=11) + base,
        'mixed-rate-frames': base[:len(head)] + b''.join(frames[:6]) + b''.join(g.stream(rng, 2, 16, 22050, 4096, 6)[0]),
        'mixed-channels-frames': base[:len(head)] + b''.join(frames[:6]) + b''.join(g.stream(rng, 1, 16, 44100, 4096, 6)[0]),
        'mixed-bps-frames': base[:len(head)] + b''.join(frames[:6]) + b''.join(g.stream(rng, 2, 24, 44100, 4096, 6)[0]),
        'long-low-rate': simple(1, 8, 8000, 4096, 150, shape='silence')[0],
    }
    for name in sorted(layouts) if seen else sorted(layouts)[::2]:
        data = layouts[name]
        if not seen and name.startswith(('junk', 'frames', 'trailing')):
            data = mutate(data, rng, 1)
        add(f'layout-{name}', data)
    full = simple(2, 16, 44100, 1152, 14)[0]
    cuts = [3, 4, 5, 8, 20, 41, 42, 43, 50, 60, 100, 1000, len(full) // 3, len(full) // 2, len(full) - 2] if seen else \
        [6, 30, 44, 47, 500, len(full) // 4, len(full) - 7]
    for cut in cuts:
        add(f'truncated-{cut}', full[:cut])
    bases = [simple(2, 16, 44100, 1152, 12)[0], simple(1, 8, 8000, 576, 14)[0], simple(6, 24, 48000, 1024, 11)[0],
             simple(2, 16, 44100, 4096, 10, metadata=[(4, g.vorbis_comment([b'TITLE=m'])), (6, g.picture())])[0],
             b''.join(g.stream(rng, 2, 16, 44100, 2048, 12)[0]), simple(2, 16, 44100, 0, 0, variable=True, sizes=[1024, 2048] * 6)[0]]
    for count in range(50 if seen else 30):
        add(f'mutated-{count}', mutate(rng.choice(bases), rng))
    if seen:
        long_frames, long_total, long_blocks = g.stream(rng, 1, 8, 8000, 4096, 150, shape='silence')
        long_sizes = [len(f) for f in long_frames]
        long_info = lambda samples: g.streaminfo(4096, 4096, min(long_sizes), max(long_sizes), 8000, 1, 8, samples)
        add('layout-long-unknown-duration', g.flac_file(long_frames, 8000, 1, 8, long_total, long_blocks, info=long_info(0)))
        add('layout-long-duration-grows', g.flac_file(long_frames, 8000, 1, 8, long_total, long_blocks, info=long_info(8000 * 20)))
    return items


def lane_items(lane):
    items = generated(lane) + flac_generated(lane)
    for index, (path, data) in enumerate(fixtures()):
        if (index % 3 == 2) == (lane == 'holdout'):
            items.append(('fixture-' + pathlib.Path(path).name.lower().replace('.', '-'), data))
    return items
