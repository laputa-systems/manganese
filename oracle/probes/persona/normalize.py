import base64
import copy
import re


def wire(entries):
    requests = {}
    hellos = []
    for entry in entries:
        if entry['kind'] == 'http':
            if entry['request'] == 'GET /favicon.ico HTTP/1.1':
                continue
            headers = []
            for name, value in entry['headers']:
                if name.lower() == 'referer':
                    if value != 'http://nav.persona.test/':
                        raise RuntimeError('unexpected referer: ' + value)
                    value = {'structure':'fixture-origin-and-root-path'}
                if name.lower() == 'cookie':
                    if value != 'persona=fixture':
                        raise RuntimeError('unexpected cookie: ' + value)
                    value = {'structure':'fixture-name-and-value'}
                headers.append([name, value])
            key = entry['request']
            value = {'host':entry['host'], 'headers':headers}
            if key in requests and requests[key] != value:
                raise RuntimeError('inconsistent repeated request: ' + key)
            requests[key] = value
        else:
            hellos.append(entry['hello'])
    required = {'GET / HTTP/1.1','GET /image HTTP/1.1','GET /script HTTP/1.1','GET /xhr HTTP/1.1','GET /fetch HTTP/1.1','GET /websocket HTTP/1.1','OPTIONS /cors HTTP/1.1','POST /cors HTTP/1.1'}
    if set(requests) != required:
        raise RuntimeError('wire request coverage: ' + repr(sorted(requests)))
    if len(hellos) < 12:
        raise RuntimeError('fewer than 12 TLS connections')
    permutations = {tuple(h['extensions']) for h in hellos}
    if len(permutations) < 2:
        raise RuntimeError('TLS extension order did not vary')
    exact = []
    for item in hellos:
        h = copy.deepcopy(item)
        order = h.pop('extensions')
        if order[0] != 'GREASE' or order[-1] != 'GREASE':
            raise RuntimeError('TLS GREASE extension positions changed')
        h['extensionSet'] = sorted(order, key=str)
        ech = h['values']['65037']
        if ech['type'] != 0 or ech['length'] not in (186,218,250,282):
            raise RuntimeError('unexpected ECH GREASE shape: ' + repr(ech))
        h['values']['65037'] = {'type':0,'lengths':[186,218,250,282]}
        exact.append(h)
    if any(h != exact[0] for h in exact[1:]):
        raise RuntimeError('TLS exact fields differ within capture: ' + repr(exact))
    return {'http':requests,'tls':{'exact':exact[0], 'varying':{'comparator':'tolerance','bound':'at least 12 connections and at least 2 distinct extension permutations; ECH length is one of 186, 218, 250, 282 bytes','relationships':['extension set and non-GREASE values are identical','GREASE extensions occupy first and last positions','GREASE values follow the 0x?a?a pattern with equal bytes','ECH outer type is zero']}}, 'http2':{'status':'unobservable-without-trust','reason':'The fixture closes after ClientHello and cannot complete TLS with a trusted certificate; no HTTP/2 frames are observable.'}}
