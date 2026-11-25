import os

import fuzzdriver


def validate(response: bytes, delay: int) -> bool:
    try:
        lines = [line.split() for line in response.decode().splitlines()]
        if len(lines) != 4 or any(len(line) != 4 or line[0] != 'E' for line in lines):
            return False
        events = [(int(line[1]), int(line[2]), int(line[3])) for line in lines]
    except (UnicodeDecodeError, ValueError):
        return False
    if {event[0] for event in events} != {1, 2, 3, 4}:
        return False
    if any(event[1:] != (delay, 1) for event in events):
        return False
    positions = {event[0]: index for index, event in enumerate(events)}
    return positions[1] < positions[2] < positions[3] and positions[2] < positions[4]


def check_partial_order() -> None:
    timeout = fuzzdriver.positive('PEER_TIMEOUT_SECS', fuzzdriver.DEFAULT_TIMEOUT_SECS)
    max_output = fuzzdriver.positive('PEER_MAX_OUTPUT', fuzzdriver.DEFAULT_MAX_OUTPUT)
    peers = []
    try:
        for role in ('oracle', 'candidate'):
            peer = fuzzdriver.Peer(role, os.environ['FUZZ_' + role.upper()], timeout, max_output)
            peers.append(peer)
            peer.handshake()
        for kind in (1, 2):
            for priority in range(3):
                for delay in (1, 7, 11):
                    request = (f'R 1 {kind} {priority} 1\n'
                               f'P 1 1 {delay} 0 0 0 0\n'
                               f'P 1 2 {delay} 1 1 4 0\n'
                               f'P 1 3 {delay} 0 0 0 0\n'
                               f'A {delay}\n').encode()
                    for peer in peers:
                        response = peer.ask(request)
                        if not validate(response, delay):
                            raise fuzzdriver.PeerFailure(f'{peer.role} violated sequence order: request={request!r} response={response!r}')
        for peer in peers:
            peer.finish()
    finally:
        for peer in peers:
            peer.stop()
    print('partial-order checks passed 18 cases', flush=True)
