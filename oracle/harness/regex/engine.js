globalThis.__regexEngine = function (P, F, S, N, I) {
  function hx(s) {
    var o = '';
    for (var i = 0; i < s.length; i++) o += ('0000' + s.charCodeAt(i).toString(16)).slice(-4);
    return o;
  }
  var flags = F.indexOf('d') < 0 ? F + 'd' : F;
  var re;
  try { re = new RegExp(P, flags); } catch (e) { return 'E'; }
  var out = ['O' + re.flags];
  re.lastIndex = I;
  for (var k = 0; k < N; k++) {
    var m;
    try { m = re.exec(S); } catch (e) { return 'X'; }
    if (m === null) { out.push('N,' + re.lastIndex); continue; }
    var idx = m.indices;
    var groups = '';
    for (var g = 1; g < m.length; g++) {
      if (g > 1) groups += ';';
      if (m[g] === undefined) groups += 'U';
      else groups += idx[g][0] + ',' + idx[g][1] + ',' + hx(m[g]);
    }
    out.push('M,' + m.index + ',' + idx[0][1] + ',' + hx(m[0]) + ',' + groups + ',' + re.lastIndex);
  }
  return out.join('|');
};
