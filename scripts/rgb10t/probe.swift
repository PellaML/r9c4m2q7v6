// Newly authored synthetic probe (public Metal API only). Adversarial rgb10a2Unorm
// bilinear rounding sweep: every 8-bit weight pair on every cell of six synthetic
// textures, plus listed boundary/sub-quantum/random coordinates. Nothing external.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ c: Bool, _ m: String) throws { if !c { throw Failure(message: m) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func bytes<T>(_ v: [T]) -> Data { v.withUnsafeBytes { Data($0) } }

let W = 16, H = 16, PARENT = 16384, OFFSET = 12288, PITCH = 256
func pack(_ r: Int, _ g: Int, _ b: Int, _ a: Int) -> UInt32 { UInt32(r) | UInt32(g) << 10 | UInt32(b) << 20 | UInt32(a) << 30 }
var state: UInt64 = 0x2545_f491_4f6c_dd1d
func rnd() -> UInt32 { state = state &* 6364136223846793005 &+ 1442695040888963407; return UInt32(truncatingIfNeeded: state >> 32) }

// ---- synthetic seed tap sets (four codes y0x0,y0x1,y1x0,y1x1) with weights (wx,wy) ----
let seeds: [([Int], Int, Int)] = [([451, 0, 719, 741], 147, 38), ([3, 510, 589, 870], 51, 85),
                                   ([757, 209, 517, 158], 159, 225), ([518, 117, 231, 804], 103, 17),
                                   ([319, 937, 705, 83], 147, 127)]
// fraction*256 of the run1 synthetic coordinates (exact binary32)
let seedFrac: [(Float, Float)] = [(Float(bitPattern: 0x4312a640), Float(bitPattern: 0x42190e00)), (Float(bitPattern: 0x424cd980), Float(bitPattern: 0x42a95e80)),
                                  (Float(bitPattern: 0x431f71e0), Float(bitPattern: 0x43611260)), (Float(bitPattern: 0x42ce44c0), Float(bitPattern: 0x4188b500)),
                                  (Float(bitPattern: 0x4312d2a0), Float(bitPattern: 0x42fd2dc0))]
struct Tile { let seed: Int; let g: Int; let kind: String; let taps: [Int]; let wx: Int; let wy: Int; let fx: Float; let fy: Float }
func transform(_ m: [Int], _ g: Int, _ wx: Int, _ wy: Int, _ fx: Float, _ fy: Float) -> ([Int], Int, Int, Float, Float) {
    var t = m, a = wx, b = wy, p = fx, q = fy
    if g & 4 != 0 { t = [t[0], t[2], t[1], t[3]]; swap(&a, &b); swap(&p, &q) }      // transpose
    if g & 1 != 0 { t = [t[1], t[0], t[3], t[2]]; a = 256 - a; p = 256 - p }        // flip x
    if g & 2 != 0 { t = [t[2], t[3], t[0], t[1]]; b = 256 - b; q = 256 - q }        // flip y
    return (t, a, b, p, q)
}
var tiles: [Tile] = []
for (si, sd) in seeds.enumerated() {
    let (m, wx, wy) = sd, (fx, fy) = seedFrac[si]
    for g in 0..<8 {
        let (t, a, b, p, q) = transform(m, g, wx, wy, fx, fy)
        tiles.append(Tile(seed: si, g: g, kind: "sym", taps: t, wx: a, wy: b, fx: p, fy: q))
        tiles.append(Tile(seed: si, g: g, kind: "complement", taps: t.map { 1023 - $0 }, wx: a, wy: b, fx: p, fy: q))
    }
    for k in 0..<4 { for d in [-1, 1] {
        var t = m; t[k] = t[k] + d; if t[k] < 0 || t[k] > 1023 { t[k] = m[k] - d }
        tiles.append(Tile(seed: si, g: 0, kind: "tap\(k)\(d > 0 ? "+" : "-")1", taps: t, wx: wx, wy: wy, fx: fx, fy: fy))
    } }
    for d in [-2, -1, 1, 2] {
        tiles.append(Tile(seed: si, g: 0, kind: "all\(d > 0 ? "+" : "")\(d)", taps: m.map { min(1023, max(0, $0 + d)) }, wx: wx, wy: wy, fx: fx, fy: fy))
    }
}
let setTiles = tiles.count // 140
while tiles.count < 192 {
    let t = (0..<4).map { _ in Int(rnd() % 1024) }
    tiles.append(Tile(seed: -1, g: 0, kind: "random", taps: t, wx: Int(rnd() % 255) + 1, wy: Int(rnd() % 255) + 1, fx: -1, fy: -1))
}
// textures 0..2: tiles (R=G=B codes), textures 3..5: random words
var texWords: [[UInt32]] = []
var tilePos: [[String: Any]] = []
for tx in 0..<3 {
    var w = [UInt32](repeating: 0, count: W * H)
    for slot in 0..<64 {
        let ti = tx * 64 + slot, t = tiles[ti], x = (slot % 8) * 2, y = (slot / 8) * 2
        for j in 0..<2 { for i in 0..<2 {
            let c = t.taps[j * 2 + i]; w[(y + j) * W + x + i] = pack(c, c, c, Int(rnd() & 3))
        } }
        tilePos.append(["tile": ti, "texture": tx, "cell_x": x, "cell_y": y, "set": t.seed, "g": t.g, "kind": t.kind,
                        "taps": t.taps, "wx": t.wx, "wy": t.wy])
    }
    texWords.append(w)
}
for _ in 3..<6 { texWords.append((0..<(W * H)).map { _ in rnd() }) }

// ---- listed coordinates (texture index, normalized uv); t = u*16-0.5 = cell + frac exactly ----
var listTex: [UInt8] = [], listUV: [SIMD2<Float>] = [], listSet: [UInt8] = []
var listSets: [[String: Any]] = []
func uvOf(_ cell: Int, _ frac: Float) -> Float { (Float(cell) + 0.5 + frac) / 16 }
var st = 0
for slot in 0..<64 { // L1 both-weight quantization boundaries (k+0.5)/256 and +-1 ulp of u, res texture 0
    let x = (slot % 8) * 2, y = (slot / 8) * 2
    let ks = [0, 1, 63, 64, 127, 128, 129, 191, 254, 255]
    for kx in ks { for ky in ks {
        let u = uvOf(x, (Float(kx) + 0.5) / 256), v = uvOf(y, (Float(ky) + 0.5) / 256)
        for uu in [u.nextDown, u, u.nextUp] { for vv in [v.nextDown, v, v.nextUp] { listTex.append(0); listUV.append(SIMD2(uu, vv)) } }
    } }
}
while listSet.count < listUV.count { listSet.append(0) }
listSets.append(["id": 0, "name": "weight_boundaries", "count": listUV.count - st]); st = listUV.count
for ti in 0..<setTiles { // L2 sub-quantum: 16x16 positions inside the rounding bin of the tile weight + the seed fraction
    let t = tiles[ti], tx = ti / 64, slot = ti % 64, x = (slot % 8) * 2, y = (slot / 8) * 2
    for my in 0..<16 { for mx in 0..<16 {
        let fx = (Float(t.wx) - 0.5 + (Float(mx) + 0.5) / 16) / 256, fy = (Float(t.wy) - 0.5 + (Float(my) + 0.5) / 16) / 256
        listTex.append(UInt8(tx)); listUV.append(SIMD2(uvOf(x, fx), uvOf(y, fy)))
    } }
    listTex.append(UInt8(tx)); listUV.append(SIMD2(uvOf(x, t.fx / 256), uvOf(y, t.fy / 256)))
}
while listSet.count < listUV.count { listSet.append(1) }
listSets.append(["id": 1, "name": "sub_quantum_in_bin_16x16_plus_seed_fraction (tile order, 257 per tile)", "count": listUV.count - st]); st = listUV.count
for tx in 0..<6 { // L3 arbitrary 24-bit fractions on every cell
    for cell in 0..<225 { for _ in 0..<64 {
        let fx = Float(rnd() >> 8) / 16777216, fy = Float(rnd() >> 8) / 16777216
        listTex.append(UInt8(tx)); listUV.append(SIMD2(uvOf(cell % 15, fx), uvOf(cell / 15, fy)))
    } }
}
while listSet.count < listUV.count { listSet.append(2) }
listSets.append(["id": 2, "name": "random_fractions_all_cells", "count": listUV.count - st])
let order = (0..<listUV.count).sorted { (listTex[$0], $0) < (listTex[$1], $1) } // stable by texture
listUV = order.map { listUV[$0] }; listTex = order.map { listTex[$0] }; listSet = order.map { listSet[$0] }
let nList = listUV.count

// exact RTNE binary32 of n/d
func f32bits(_ n: UInt64, _ d: UInt64) -> UInt32 {
    if n == 0 { return 0 }
    var shift: UInt32 = 0
    while (n << shift) < (d << 23) { shift += 1 }
    let scaled = n << shift
    var q = scaled / d; let r = scaled % d
    if r * 2 > d || (r * 2 == d && q & 1 == 1) { q += 1 }
    var e = 150 - shift
    if q == 1 << 24 { q >>= 1; e += 1 }
    return (e << 23) | UInt32(q & 0x7f_ffff)
}

func run() throws -> [String: Any] {
    try need(CommandLine.arguments.count == 3, "usage: probe probe.metal outdir")
    let out = URL(fileURLWithPath: CommandLine.arguments[2])
    let source = try String(contentsOfFile: CommandLine.arguments[1], encoding: .utf8)
    guard let device = MTLCreateSystemDefaultDevice() else { throw Failure(message: "UNAVAILABLE: no device") }
    guard let queue = device.makeCommandQueue() else { throw Failure(message: "queue") }
    let opts = MTLCompileOptions()
    if #available(macOS 15.0, *) { opts.mathMode = .safe } else { opts.fastMathEnabled = false }
    let lib = try device.makeLibrary(source: source, options: opts)
    guard let gf = lib.makeFunction(name: "grid"), let lf = lib.makeFunction(name: "list") else { throw Failure(message: "functions") }
    let gridPSO = try device.makeComputePipelineState(function: gf), listPSO = try device.makeComputePipelineState(function: lf)

    let sd = MTLSamplerDescriptor()
    sd.minFilter = .linear; sd.magFilter = .linear; sd.mipFilter = .notMipmapped
    sd.normalizedCoordinates = true; sd.maxAnisotropy = 1; sd.lodMinClamp = 0; sd.lodMaxClamp = Float.greatestFiniteMagnitude
    sd.sAddressMode = .clampToEdge; sd.tAddressMode = .clampToEdge; sd.rAddressMode = .clampToEdge; sd.compareFunction = .never
    try need(sd.responds(to: NSSelectorFromString("setLodAverage:")), "API_UNAVAILABLE: lodAverage")
    sd.setValue(true, forKey: "lodAverage")
    try need((sd.value(forKey: "lodAverage") as? Bool) == true, "lodAverage readback")
    guard let sampler = device.makeSamplerState(descriptor: sd) else { throw Failure(message: "sampler") }

    func shared(_ d: Data) throws -> MTLBuffer {
        try d.withUnsafeBytes { (p: UnsafeRawBufferPointer) throws -> MTLBuffer in
            guard let b = device.makeBuffer(bytes: p.baseAddress!, length: p.count, options: .storageModeShared) else { throw Failure(message: "buffer") }
            return b }
    }
    func zeroed(_ len: Int) throws -> MTLBuffer {
        guard let b = device.makeBuffer(length: len, options: .storageModeShared) else { throw Failure(message: "buffer \(len)") }
        memset(b.contents(), 0, len); return b
    }
    func finish(_ cb: MTLCommandBuffer) throws {
        cb.commit(); cb.waitUntilCompleted()
        try need(cb.status == .completed && cb.error == nil, "status \(cb.status.rawValue) \(String(describing: cb.error))")
    }
    let t4092 = try shared(bytes((0...4092).map { f32bits(UInt64($0), 4092) }))
    let t4080 = try shared(bytes((0...4080).map { f32bits(UInt64($0), 4080) }))
    let uvBuf = try shared(bytes(listUV))

    // dump slots: the five identity (g=0, sym) seed tiles
    var dumpSlots = [[Int32]](repeating: [Int32](repeating: -1, count: 225), count: 6)
    var dumpInfo: [[String: Any]] = []
    for (ti, t) in tiles.enumerated() where t.kind == "sym" && t.g == 0 {
        let tx = ti / 64, slot = ti % 64, cell = (slot / 8) * 2 * 15 + (slot % 8) * 2
        dumpSlots[tx][cell] = Int32(dumpInfo.count); dumpInfo.append(["slot": dumpInfo.count, "tile": ti, "texture": tx, "cell": cell, "set": t.seed])
    }
    let dumpSlotBufs = try dumpSlots.map { try shared(bytes($0)) }

    let usage: MTLTextureUsage = [.shaderRead, .shaderWrite, .renderTarget]
    func makeTexture(_ variant: String, _ words: [UInt32]) throws -> MTLTexture {
        var parent = [UInt8](repeating: 0xa5, count: PARENT)
        for y in 0..<H { for x in 0..<W { for b in 0..<4 { parent[OFFSET + y * PITCH + x * 4 + b] = UInt8((words[y * W + x] >> (8 * UInt32(b))) & 0xff) } } }
        let staging = try shared(Data(parent))
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgb10a2Unorm, width: W, height: H, mipmapped: false)
        d.usage = usage
        if variant == "view-private" {
            d.storageMode = .private
            guard let pb = device.makeBuffer(length: PARENT, options: .storageModePrivate),
                  let cb = queue.makeCommandBuffer(), let e = cb.makeBlitCommandEncoder() else { throw Failure(message: "private parent") }
            e.copy(from: staging, sourceOffset: 0, to: pb, destinationOffset: 0, size: PARENT); e.endEncoding(); try finish(cb)
            guard let t = pb.makeTexture(descriptor: d, offset: OFFSET, bytesPerRow: PITCH) else { throw Failure(message: "view texture") }
            return t
        }
        d.storageMode = .shared
        guard let t = device.makeTexture(descriptor: d) else { throw Failure(message: "std texture") }
        bytes(words).withUnsafeBytes { t.replace(region: MTLRegionMake2D(0, 0, W, H), mipmapLevel: 0, withBytes: $0.baseAddress!, bytesPerRow: W * 4) }
        return t
    }

    let nearCap = 1 << 20, misCap = 1 << 18
    var files: [String: String] = [:]
    func save(_ name: String, _ d: Data) throws { try d.write(to: out.appendingPathComponent(name)); files[name] = sha(d) }
    var tw = Data(); for w in texWords { tw.append(bytes(w)) }
    try save("textures-6x256-u32.bin", tw); try save("list-uv-f32x2.bin", bytes(listUV)); try save("list-tex-u8.bin", Data(listTex)); try save("list-set-u8.bin", Data(listSet))
    var variants: [[String: Any]] = []
    var ref: (mis: Data, hist: Data, list: Data, dump: Data)? = nil
    for variant in ["view-private", "std-shared"] {
        let t0 = Date()
        let ctr = try zeroed(4096 * 4), mis = try zeroed(misCap * 8), near = try zeroed(nearCap * 8)
        let dump = try zeroed(dumpInfo.count * 65536 * 8), listOut = try zeroed(nList * 16)
        memset(listOut.contents(), 0xde, nList * 16)
        var info: [String: Any] = ["variant": variant]
        for tx in 0..<6 {
            let tex = try makeTexture(variant, texWords[tx])
            if tx == 0 { let ti: [String: Any] = ["storageMode": tex.storageMode.rawValue, "usage": tex.usage.rawValue, "isBufferView": tex.buffer != nil,
                                            "bufferOffset": tex.buffer != nil ? tex.bufferOffset : -1, "bufferBytesPerRow": tex.buffer != nil ? tex.bufferBytesPerRow : -1]; info["texture"] = ti }
            let wb = try shared(bytes(texWords[tx]))
            for row in 0..<15 {
                var gp: [UInt32] = [UInt32(tx), UInt32(row * 15), UInt32(nearCap), UInt32(misCap)]
                guard let cb = queue.makeCommandBuffer(), let e = cb.makeComputeCommandEncoder() else { throw Failure(message: "enc") }
                e.setComputePipelineState(gridPSO); e.setTexture(tex, index: 0); e.setSamplerState(sampler, index: 0)
                e.setBuffer(wb, offset: 0, index: 0); e.setBytes(&gp, length: 16, index: 1)
                e.setBuffer(t4092, offset: 0, index: 2); e.setBuffer(t4080, offset: 0, index: 3)
                e.setBuffer(ctr, offset: 0, index: 4); e.setBuffer(mis, offset: 0, index: 5); e.setBuffer(near, offset: 0, index: 6)
                e.setBuffer(dump, offset: 0, index: 7); e.setBuffer(dumpSlotBufs[tx], offset: 0, index: 8)
                e.dispatchThreadgroups(MTLSize(width: 15 * 65536 / 256, height: 1, depth: 1), threadsPerThreadgroup: MTLSize(width: 256, height: 1, depth: 1))
                e.endEncoding(); try finish(cb)
            }
            let idx = (0..<nList).filter { Int(listTex[$0]) == tx }
            if !idx.isEmpty {
                let lo = idx.first!, cnt = idx.count
                try need(idx.last! - lo + 1 == cnt, "list not contiguous")
                var nn = UInt32(cnt)
                guard let cb = queue.makeCommandBuffer(), let e = cb.makeComputeCommandEncoder() else { throw Failure(message: "enc") }
                e.setComputePipelineState(listPSO); e.setTexture(tex, index: 0); e.setSamplerState(sampler, index: 0)
                e.setBytes(&nn, length: 4, index: 0); e.setBuffer(uvBuf, offset: lo * 8, index: 1); e.setBuffer(listOut, offset: lo * 16, index: 2)
                e.dispatchThreadgroups(MTLSize(width: (cnt + 255) / 256, height: 1, depth: 1), threadsPerThreadgroup: MTLSize(width: 256, height: 1, depth: 1))
                e.endEncoding(); try finish(cb)
            }
        }
        let c = ctr.contents().bindMemory(to: UInt32.self, capacity: 4096)
        let misN = Int(c[0]), nearN = Int(c[1])
        info["lanes_grid"] = 6 * 225 * 65536 * 4
        info["mismatch_lanes"] = misN; info["near_tie_records"] = nearN; info["offgrid_lanes"] = Int(c[2])
        info["mismatch_truncated"] = misN > misCap; info["near_truncated"] = nearN > nearCap
        info["seconds"] = Date().timeIntervalSince(t0)
        // records are appended in nondeterministic order: sort for comparison
        func sortedRecords(_ b: MTLBuffer, _ n: Int) -> Data {
            let p = b.contents().bindMemory(to: UInt64.self, capacity: n)
            return bytes(Array(UnsafeBufferPointer(start: p, count: n)).sorted())
        }
        let misD = sortedRecords(mis, min(misN, misCap))
        let histD = Data(bytes: ctr.contents() + 16 * 4, count: (3 * 257 * 3 + 65 * 3) * 4)
        let listD = Data(bytes: listOut.contents(), count: nList * 16)
        let dumpD = Data(bytes: dump.contents(), count: dumpInfo.count * 65536 * 8)
        info["sha256"] = ["mismatch_sorted": sha(misD), "hist": sha(histD), "list": sha(listD), "dump": sha(dumpD)]
        let tag = variant
        if ref == nil {
            ref = (misD, histD, listD, dumpD)
            try save("grid-mismatch-\(tag)-u32x2.bin", misD); try save("grid-hist-\(tag)-u32.bin", histD)
            try save("list-out-\(tag)-u32x4.bin", listD); try save("grid-dump-\(tag)-u16x4.bin", dumpD)
            try save("grid-near-\(tag)-u32x2.bin", sortedRecords(near, min(nearN, nearCap)))
        } else {
            info["equal_to_view_private"] = ["mismatch": misD == ref!.mis, "hist": histD == ref!.hist, "list": listD == ref!.list, "dump": dumpD == ref!.dump]
            if misD != ref!.mis { try save("grid-mismatch-\(tag)-u32x2.bin", misD) }
            if histD != ref!.hist { try save("grid-hist-\(tag)-u32.bin", histD) }
            if listD != ref!.list { try save("list-out-\(tag)-u32x4.bin", listD) }
            if dumpD != ref!.dump { try save("grid-dump-\(tag)-u16x4.bin", dumpD) }
        }
        variants.append(info)
    }
    var dev: [String: Any] = ["name": device.name, "registryID": device.registryID, "hasUnifiedMemory": device.hasUnifiedMemory,
                              "supportsMac2": device.supportsFamily(.mac2), "supportsApple7": device.supportsFamily(.apple7)]
    if #available(macOS 14.0, *) { dev["architecture"] = device.architecture.name }
    var res: [String: Any] = [:]
    let allEq = variants.count == 2 && ((variants[1]["equal_to_view_private"] as? [String: Bool])?.values.allSatisfy { $0 } ?? false)
    res["status"] = allEq ? "COMPLETED_VARIANTS_EQUAL" : "COMPLETED_VARIANTS_DIFFER"
    res["device"] = dev; res["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    res["variants"] = variants; res["tiles"] = tilePos; res["dump_slots"] = dumpInfo; res["list_sets"] = listSets
    res["list_count"] = nList; res["files_sha256"] = files
    res["sampler"] = "linear min/mag, notMipmapped, normalized, clampToEdge STR, aniso 1, lod 0..FLT_MAX, compare never, lodAverage true; sample(s,uv,level(0)) compute"
    res["layout"] = [
        "grid id": "tex<<28 | cell<<16 | kx<<8 | ky; cell = cy*15+cx; uv = ((cx*256+128+kx)/4096, (cy*256+128+ky)/4096) so weights are exactly kx,ky",
        "record": "u32 pair (id, ch<<30 | (n_gpu - floor(X/65536) + 512)<<16 | (X mod 65536)); X = 4*sum(W*c) RGB or 16*sum(W*85a) alpha; delta field 0x3ff = off n/den grid",
        "mismatch": "lanes where n_gpu != floor((X+32768)/65536) or off-grid; sorted",
        "near": "RGB lanes with |X mod 65536 - 32768| <= 48, all outcomes; sorted",
        "hist": "u32[3][257][3] RGB by (eps+512)/4 for |eps|<=512 x outcome(down,up,other) then u32[65][3] alpha by (eps+512)/16",
        "dump": "u16x4 n_gpu (0xffff off-grid) per dump slot, index kx*256+ky",
        "list": "raw binary32 RGBA bits per listed uv (texture index in list-tex-u8.bin)"]
    return res
}
let outDir = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "."
var result: [String: Any]
var code: Int32 = 0
do { result = try run() } catch let e as Failure {
    result = ["status": e.message.hasPrefix("UNAVAILABLE") ? "UNAVAILABLE_NOT_PASS" : "FAILED_NOT_PASS", "error": e.message]; code = 3
} catch { result = ["status": "FAILED_NOT_PASS", "error": "\(error)"]; code = 3 }
let json = try! JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
try! json.write(to: URL(fileURLWithPath: outDir).appendingPathComponent("summary.json"))
FileHandle.standardOutput.write(json.prefix(3000))
exit(code)