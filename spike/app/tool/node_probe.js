// Headless stand-in for the browser row of the spike: load the exact web build
// (build/web/pkg: glue JS + WASM, non-shared memory) in Node and call the
// flutter_rust_bridge sync dispatcher the way the Dart web runtime does.
// Usage: node tool/node_probe.js build/web/pkg ../vectors/op_vector_v1.json
// Func ids come from rust/src/frb_generated.rs; regenerate => recheck them.
const fs = require('fs'), crypto = require('crypto');
const dir = process.argv[2];
const glue = fs.readFileSync(dir + '/rust_lib_app.js', 'utf8');
const wb = new Function(glue + '\n;return wasm_bindgen;')();
const t0 = process.hrtime.bigint();
wb.initSync(fs.readFileSync(dir + '/rust_lib_app_bg.wasm'));
const tInit = Number(process.hrtime.bigint() - t0) / 1e6;
const v = JSON.parse(fs.readFileSync(process.argv[3], 'utf8'));
const hex = s => Uint8Array.from(s.match(/../g).map(h => parseInt(h, 16)));
function sseList(bytes) { const b = new Uint8Array(4 + bytes.length); new DataView(b.buffer).setInt32(0, bytes.length, true); b.set(bytes, 4); return b; }
const args = sseList(hex(v.signed));
const t1 = process.hrtime.bigint();
const ret = wb.frb_pde_ffi_dispatcher_sync(3, args, args.length, args.length);
const tCall = Number(process.hrtime.bigint() - t1) / 1e6;
console.log('ret type', Object.prototype.toString.call(ret), ret && ret.length);
const bytes = ret instanceof Uint8Array ? ret : (Array.isArray(ret) ? ret[0] : ret);
const hx = Buffer.from(bytes).toString('hex');
console.log('expect id', v.id, 'present:', hx.includes(v.id));
console.log('node sha256 of signed', crypto.createHash('sha256').update(hex(v.signed)).digest('hex'));
console.log(`init ms ${tInit.toFixed(2)} firstcall ms ${tCall.toFixed(3)}`);
const ok = hx === '0020000000' + v.id;
// verify_op (func 6): untampered -> true, one flipped body byte -> false.
const call6 = b => { const a = sseList(b); return Buffer.from(wb.frb_pde_ffi_dispatcher_sync(6, a, a.length, a.length)).toString('hex'); };
const good = hex(v.signed), bad = hex(v.signed); bad[bad.length - 68] ^= 1;
const vGood = call6(good), vBad = call6(bad);
console.log('verify_op good', vGood, 'tampered', vBad);
const pass = ok && vGood === '0001' && vBad === '0000';
console.log(pass ? 'PASS' : 'FAIL');
process.exit(pass ? 0 : 1);
