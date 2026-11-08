// Public Metal API measurement of synthetic scalar textures; no CPU result path.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ condition: Bool, _ message: String) throws {
    if !condition { throw Failure(message: message) }
}
func sha(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}
func errorInfo(_ error: Error) -> [String: Any] {
    if let error = error as? Failure { return ["reason": error.message] }
    let e = error as NSError
    // Do not serialize arbitrary native error descriptions, URLs, or userInfo.
    return ["reason": "native_api_error", "domain": e.domain, "code": e.code]
}
func u32(_ bytes: [UInt8], _ offset: Int) -> UInt32 {
    UInt32(bytes[offset]) | UInt32(bytes[offset + 1]) << 8 |
        UInt32(bytes[offset + 2]) << 16 | UInt32(bytes[offset + 3]) << 24
}
func u16(_ bytes: [UInt8], _ offset: Int) -> UInt16 {
    UInt16(bytes[offset]) | UInt16(bytes[offset + 1]) << 8
}

struct CaseSpec: Decodable {
    let name: String
    let width: Int
    let height: Int
    let centerCount: Int
    let rowCount: Int
    let seedFile: String
    let uvFile: String
}
struct Manifest: Decodable {
    let schema: Int
    let totalRows: Int
    let cases: [CaseSpec]
}

final class Store {
    let out: URL
    var hashes: [String: String] = [:]
    var bytes = 0
    init(_ out: URL) { self.out = out }
    func save(_ name: String, _ data: Data) throws -> String {
        try need(!name.contains("/") && !name.contains("\\"), "invalid output name")
        try need(hashes[name] == nil, "duplicate output name")
        try need(data.count <= 1_048_576 && bytes + data.count <= 6 * 1024 * 1024,
                 "raw output byte budget exceeded")
        try need(hashes.count < 192, "raw output count budget exceeded")
        try data.write(to: out.appendingPathComponent(name), options: .withoutOverwriting)
        bytes += data.count
        hashes[name] = sha(data)
        return name
    }
}

struct Health {
    var poisonWords = 0
    var poisonRows: [Int] = []
    var markerRows: [Int] = []
    var badGuardBytes = 0
    var clean: Bool {
        poisonWords == 0 && markerRows.isEmpty && badGuardBytes == 0
    }
    var json: [String: Any] {
        ["poison_words": poisonWords, "poison_rows": poisonRows.count,
         "bad_marker_rows": markerRows.count, "bad_guard_bytes": badGuardBytes,
         "first_poison_rows": Array(poisonRows.prefix(8)),
         "first_bad_marker_rows": Array(markerRows.prefix(8))]
    }
}

final class Plane {
    let name: String
    let count: Int
    let stride: Int
    let laneBytes: Int
    let markerTag: UInt32?
    let buffer: MTLBuffer
    let guardBytes = 64
    var size: Int { count * stride }
    init(_ dev: MTLDevice, _ name: String, _ count: Int, _ laneBytes: Int,
         markerTag: UInt32? = nil) throws {
        self.name = name; self.count = count; self.laneBytes = laneBytes
        self.stride = 4 * laneBytes; self.markerTag = markerTag
        guard let b = dev.makeBuffer(length: count * 4 * laneBytes + 64,
                                     options: .storageModeShared) else {
            throw Failure(message: "result buffer allocation failed")
        }
        buffer = b
        memset(b.contents(), 0xcd, b.length)
        try need(b.storageMode == .shared, "result buffer storage mismatch")
    }
    func raw() -> Data { Data(bytes: buffer.contents(), count: size) }
    func health() -> Health {
        let b = [UInt8](Data(bytes: buffer.contents(), count: size + guardBytes))
        var result = Health()
        for i in 0..<count {
            var poison = false
            for lane in 0..<4 {
                let offset = i * stride + lane * laneBytes
                let unchanged = laneBytes == 4 ? u32(b, offset) == 0xcdcdcdcd :
                    u16(b, offset) == 0xcdcd
                if unchanged { result.poisonWords += 1; poison = true }
            }
            if poison { result.poisonRows.append(i) }
            if let tag = markerTag {
                let offset = i * stride
                if u32(b, offset) != 0xa801c0de || u32(b, offset + 4) != UInt32(i) ||
                    u32(b, offset + 8) != tag || u32(b, offset + 12) != ~UInt32(i) {
                    result.markerRows.append(i)
                }
            }
        }
        result.badGuardBytes = b[size...].filter { $0 != 0xcd }.count
        return result
    }
}

struct Capture {
    let planes: [Plane]
    func archive(_ prefix: String, _ store: Store) throws -> [String: Any] {
        var result: [String: Any] = [:]
        for p in planes {
            result[p.name] = ["file": try store.save(prefix + "-" + p.name + ".bin", p.raw()),
                              "rows": p.count, "stride_bytes": p.stride,
                              "health": p.health().json]
        }
        return result
    }
    func requireClean() throws {
        for p in planes { try need(p.health().clean, "poison, marker, or guard failure: " + p.name) }
    }
}

var report: [String: Any] = ["schema": 1, "status": "FAILED_NOT_PASS"]
var commands: [[String: Any]] = []
var observations: [[String: Any]] = []
var errors: [[String: Any]] = []
var inputs: [String: String] = [:]
var negativeComplete = false

func run(_ source: URL, _ store: Store) throws {
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little-endian host required")
    let root = source.deletingLastPathComponent()
    var sources: [String: String] = [:]
    for name in ["cases.rs", "probe.swift", "probe.metal", "run.sh"] {
        sources[name] = sha(try Data(contentsOf: root.appendingPathComponent(name)))
    }
    report["source_sha256"] = sources
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    report["toolchain_receipt"] = "../run-receipt.txt"
    report["layout"] = ["float": "raw GPU uint4: little-endian RGBA binary32 bits",
                        "half": "raw GPU ushort4: little-endian RGBA binary16 bits",
                        "markers": "little-endian uint4: 0xa801c0de, row, dispatch tag, ~row",
                        "guards": "64 trailing bytes of 0xcd on every result buffer; not in raw files"]
    report["semantics"] = ["a8": "enum 1, one-byte UNORM; RGBA=(0,0,0,a/255)",
                           "r8": "enum 10, independent control; RGBA=(r/255,0,0,1)",
                           "linear_numerical_assertions": false,
                           "float_unorm_absolute_tolerance": 0.000001,
                           "half_unorm_absolute_tolerance": 0.0005,
                           "anchors": "zero channels and UNORM 0/255 endpoints checked bit-exactly"]

    func input(_ name: String) throws -> Data {
        let d = try Data(contentsOf: store.out.appendingPathComponent(name))
        try need(d.count <= 131072, "input size budget exceeded")
        inputs[name] = sha(d)
        return d
    }
    let manifest = try JSONDecoder().decode(Manifest.self, from: input("cases.json"))
    try need(manifest.schema == 1 && manifest.cases.count == 5 &&
             manifest.totalRows <= 8192 && manifest.totalRows >= 7000, "input manifest bounds")
    try need(manifest.cases.map { $0.name } == ["ramp", "tile2", "tile7", "lineh", "linev"],
             "input case names/order")
    try need(manifest.cases.reduce(0) { $0 + $1.rowCount } == manifest.totalRows, "row total mismatch")
    report["coordinate_rows"] = manifest.totalRows
    report["read_rows_per_format_storage"] = manifest.cases.reduce(0) { $0 + $1.centerCount }

    guard let dev = MTLCreateSystemDefaultDevice(), let queue = dev.makeCommandQueue() else {
        throw Failure(message: "Metal device or command queue unavailable")
    }
    var families: [String: Bool] = [:]
    let knownFamilies: [(String, MTLGPUFamily)] = [
        ("apple1", .apple1), ("apple2", .apple2), ("apple3", .apple3),
        ("apple4", .apple4), ("apple5", .apple5), ("apple6", .apple6), ("apple7", .apple7),
        ("mac1", .mac1), ("mac2", .mac2),
        ("common1", .common1), ("common2", .common2), ("common3", .common3)
    ]
    for (name, family) in knownFamilies { families[name] = dev.supportsFamily(family) }
    if #available(macOS 13.0, *) {
        families["apple8"] = dev.supportsFamily(.apple8)
        families["metal3"] = dev.supportsFamily(.metal3)
    }
    if #available(macOS 14.0, *) { families["apple9"] = dev.supportsFamily(.apple9) }
    var deviceInfo: [String: Any] = ["name": dev.name, "registry_id": String(dev.registryID),
        "has_unified_memory": dev.hasUnifiedMemory, "is_low_power": dev.isLowPower,
        "is_removable": dev.isRemovable, "family_queries": families,
        "family_queries_exhaustive": false]
    if #available(macOS 14.0, *) { deviceInfo["architecture"] = dev.architecture.name }
    report["device"] = deviceInfo
    try need(MTLPixelFormat.a8Unorm.rawValue == 1 && MTLPixelFormat.r8Unorm.rawValue == 10,
             "pixel format enum mismatch")

    let options = MTLCompileOptions()
    options.fastMathEnabled = false
    report["compiler_options"] = ["fastMathEnabled": options.fastMathEnabled,
        "languageVersion": options.languageVersion.rawValue, "language_selection": "SDK default",
        "swift_flags": ["-swift-version", "5", "-O"], "cpu_fallback": false]
    let library = try dev.makeLibrary(source: String(contentsOf: source, encoding: .utf8), options: options)
    func pipeline(_ name: String) throws -> MTLComputePipelineState {
        guard let f = library.makeFunction(name: name) else { throw Failure(message: "kernel missing: " + name) }
        return try dev.makeComputePipelineState(function: f)
    }
    let reader = try pipeline("read_pair"), sampler = try pipeline("sample_pair")
    report["pipelines"] = ["read_max_threads": reader.maxTotalThreadsPerThreadgroup,
                           "sample_max_threads": sampler.maxTotalThreadsPerThreadgroup]

    var samplerInfo: [[String: Any]] = []
    func makeSampler(_ linear: Bool) throws -> MTLSamplerState {
        let d = MTLSamplerDescriptor()
        d.minFilter = linear ? .linear : .nearest; d.magFilter = d.minFilter
        d.mipFilter = .notMipmapped; d.normalizedCoordinates = true
        d.sAddressMode = .clampToEdge; d.tAddressMode = .clampToEdge; d.rAddressMode = .clampToEdge
        d.maxAnisotropy = 1; d.lodMinClamp = 0; d.lodMaxClamp = 0
        guard let s = dev.makeSamplerState(descriptor: d) else { throw Failure(message: "sampler unavailable") }
        samplerInfo.append(["min": d.minFilter.rawValue, "mag": d.magFilter.rawValue,
            "mip": d.mipFilter.rawValue, "normalized": d.normalizedCoordinates,
            "address_str": [d.sAddressMode.rawValue, d.tAddressMode.rawValue, d.rAddressMode.rawValue],
            "anisotropy": d.maxAnisotropy, "lod_min_bits": d.lodMinClamp.bitPattern,
            "lod_max_bits": d.lodMaxClamp.bitPattern])
        return s
    }
    let near = try makeSampler(false), linear = try makeSampler(true)
    report["samplers"] = samplerInfo

    func buffer(_ d: Data) throws -> MTLBuffer {
        try d.withUnsafeBytes { p throws -> MTLBuffer in
            guard let base = p.baseAddress,
                  let b = dev.makeBuffer(bytes: base, length: p.count, options: .storageModeShared) else {
                throw Failure(message: "input buffer allocation failed")
            }
            try need(b.storageMode == .shared, "input buffer storage mismatch")
            return b
        }
    }
    func finish(_ cb: MTLCommandBuffer, _ label: String) throws {
        cb.commit(); cb.waitUntilCompleted()
        var receipt: [String: Any] = ["label": label, "status": cb.status.rawValue,
                                     "completed": cb.status == .completed, "error_present": cb.error != nil]
        if let error = cb.error { receipt["error"] = errorInfo(error) }
        commands.append(receipt)
        try need(cb.status == .completed && cb.error == nil, "GPU command did not complete cleanly: " + label)
    }
    func capture(_ texture: MTLTexture, _ uv: MTLBuffer, _ readN: Int, _ sampleN: Int,
                 _ tag: UInt32, _ skip: UInt32, _ label: String) throws -> Capture {
        let rf = try Plane(dev, "read-f32", readN, 4)
        let rh = try Plane(dev, "read-f16", readN, 2)
        let rm = try Plane(dev, "read-markers", readN, 4, markerTag: tag)
        let nf = try Plane(dev, "near-f32", sampleN, 4)
        let nh = try Plane(dev, "near-f16", sampleN, 2)
        let lf = try Plane(dev, "linear-f32", sampleN, 4)
        let lh = try Plane(dev, "linear-f16", sampleN, 2)
        let sm = try Plane(dev, "sample-markers", sampleN, 4, markerTag: tag + 1)
        guard let cb = queue.makeCommandBuffer(), let e = cb.makeComputeCommandEncoder() else {
            throw Failure(message: "compute command allocation failed")
        }
        func dispatch(_ pso: MTLComputePipelineState, _ count: Int) {
            let threads = min(64, pso.maxTotalThreadsPerThreadgroup)
            e.dispatchThreadgroups(MTLSize(width: (count + threads - 1) / threads, height: 1, depth: 1),
                                   threadsPerThreadgroup: MTLSize(width: threads, height: 1, depth: 1))
        }
        e.setComputePipelineState(reader)
        e.setTexture(texture, index: 0); e.setTexture(texture, index: 1)
        for (i, p) in [rf, rh, rm].enumerated() { e.setBuffer(p.buffer, offset: 0, index: i) }
        var rp = SIMD4<UInt32>(UInt32(readN), tag, skip, 0)
        e.setBytes(&rp, length: 16, index: 3); dispatch(reader, readN)
        e.setComputePipelineState(sampler)
        e.setSamplerState(near, index: 0); e.setSamplerState(linear, index: 1)
        e.setBuffer(uv, offset: 0, index: 0)
        for (i, p) in [nf, nh, lf, lh, sm].enumerated() { e.setBuffer(p.buffer, offset: 0, index: i + 1) }
        var sp = SIMD4<UInt32>(UInt32(sampleN), tag + 1, skip, 0)
        e.setBytes(&sp, length: 16, index: 6); dispatch(sampler, sampleN)
        e.endEncoding(); try finish(cb, label)
        return Capture(planes: [rf, rh, rm, nf, nh, lf, lh, sm])
    }

    func semanticChecks(_ cap: Capture, _ seed: [UInt8], _ alpha: Bool) throws -> [String: Any] {
        let scalarLane = alpha ? 3 : 0
        for (fIndex, hIndex) in [(0, 1), (3, 4)] { // read and the nearest center prefix only
            let f = [UInt8](cap.planes[fIndex].raw()), h = [UInt8](cap.planes[hIndex].raw())
            for (i, code) in seed.enumerated() {
                for lane in 0..<4 {
                    let fb = u32(f, i * 16 + lane * 4), hb = u16(h, i * 8 + lane * 2)
                    if lane == scalarLane {
                        let fv = Float(bitPattern: fb), hv = Float(Float16(bitPattern: hb))
                        let expected = Float(code) / 255
                        try need(fv.isFinite && hv.isFinite && fv >= 0 && fv <= 1 && hv >= 0 && hv <= 1 &&
                                 abs(fv - expected) <= 0.000001 && abs(hv - expected) <= 0.0005,
                                 "UNORM code check failed at row \(i)")
                        if code == 0 { try need(fb == 0 && hb == 0, "zero endpoint mismatch") }
                        if code == 255 { try need(fb == 0x3f800000 && hb == 0x3c00, "one endpoint mismatch") }
                    } else {
                        let one = !alpha && lane == 3
                        try need(fb == (one ? 0x3f800000 : 0) && hb == (one ? 0x3c00 : 0),
                                 "documented channel mapping mismatch at row \(i)")
                    }
                }
            }
        }
        return ["read_rows": seed.count, "nearest_center_rows": seed.count,
                "float_and_half_checked": true, "channel_mapping_checked": true,
                "endpoint_anchors_checked": true, "linear_values_checked": false]
    }

    let formats: [(String, MTLPixelFormat)] = [("a8", .a8Unorm), ("r8", .r8Unorm)]
    // Separate Managed-only follow-up; do not silently substitute storage modes.
    let modes: [(String, MTLStorageMode)] = [("managed", .managed)]
    var serial: UInt32 = 0
    for (formatName, format) in formats {
        for (modeName, mode) in modes {
            for c in manifest.cases {
                serial += 1
                let label = formatName + "-" + modeName + "-" + c.name
                var obs: [String: Any] = ["name": label, "status": "FAILED_NOT_PASS",
                    "case": c.name, "requested_format": format.rawValue,
                    "requested_storage": mode.rawValue, "requested_storage_name": modeName,
                    "read_rows": c.centerCount, "sample_rows": c.rowCount, "dispatch_tag": serial * 2]
                do {
                    try need(c.width > 0 && c.width <= 16 && c.height > 0 && c.height <= 16 &&
                             c.centerCount == c.width * c.height && c.rowCount >= c.centerCount &&
                             c.rowCount <= 4096, "case bounds")
                    try need(c.seedFile == c.name + "-seed.bin" && c.uvFile == c.name + "-uv.bin", "input filenames")
                    let seedData = try input(c.seedFile), uvData = try input(c.uvFile)
                    let seed = [UInt8](seedData)
                    try need(seed.count == c.width * c.height && uvData.count == c.rowCount * 8, "input lengths")
                    if c.name == "ramp" {
                        try need(c.width == 16 && c.height == 16 && seed == (0...255).map { UInt8($0) },
                                 "ramp must contain all 256 codes in order")
                    }
                    let uv = try buffer(uvData)
                    let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: format, width: c.width,
                                                                    height: c.height, mipmapped: false)
                    d.storageMode = mode; d.usage = .shaderRead
                    d.swizzle = MTLTextureSwizzleChannels(red: .red, green: .green, blue: .blue, alpha: .alpha)
                    obs["descriptor_resource_options"] = d.resourceOptions.rawValue
                    guard let t = dev.makeTexture(descriptor: d) else {
                        throw Failure(message: "requested texture format/storage allocation failed; no substitution")
                    }
                    let sw = t.swizzle
                    obs["actual_texture"] = ["format": t.pixelFormat.rawValue, "storage": t.storageMode.rawValue,
                        "usage": t.usage.rawValue, "type": t.textureType.rawValue,
                        "width": t.width, "height": t.height, "depth": t.depth,
                        "mips": t.mipmapLevelCount, "array_length": t.arrayLength, "samples": t.sampleCount,
                        "swizzle_rgba": [sw.red.rawValue, sw.green.rawValue, sw.blue.rawValue, sw.alpha.rawValue],
                        "buffer_backed": t.buffer != nil, "framebuffer_only": t.isFramebufferOnly,
                        "cpu_cache_mode": t.cpuCacheMode.rawValue, "hazard_tracking_mode": t.hazardTrackingMode.rawValue]
                    try need(t.pixelFormat == format && t.storageMode == mode && t.usage == .shaderRead &&
                             t.textureType == .type2D && t.width == c.width && t.height == c.height && t.depth == 1 &&
                             t.mipmapLevelCount == 1 && t.arrayLength == 1 && t.sampleCount == 1 && t.buffer == nil &&
                             sw.red == .red && sw.green == .green && sw.blue == .blue && sw.alpha == .alpha,
                             "actual texture properties differ from requested properties")
                    if mode == .private || mode == .managed {
                        let pitch = ((c.width + 255) / 256) * 256 + 256
                        let offset = 256, length = offset + pitch * c.height + 256
                        var bytes = [UInt8](repeating: 0xa5, count: length)
                        for y in 0..<c.height {
                            for x in 0..<c.width { bytes[offset + y * pitch + x] = seed[y * c.width + x] }
                        }
                        let padded = Data(bytes), upload = try buffer(padded)
                        obs["upload"] = ["method": "buffer-to-" + modeName + "-texture-blit", "bytes_per_texel": 1,
                            "offset": offset, "bytes_per_row": pitch, "bytes_per_image": pitch * c.height,
                            "length": length, "padding_byte": 165, "sha256": sha(padded),
                            "source_storage": upload.storageMode.rawValue]
                        guard let cb = queue.makeCommandBuffer(), let e = cb.makeBlitCommandEncoder() else {
                            throw Failure(message: "upload command allocation failed")
                        }
                        e.copy(from: upload, sourceOffset: offset, sourceBytesPerRow: pitch,
                               sourceBytesPerImage: pitch * c.height,
                               sourceSize: MTLSize(width: c.width, height: c.height, depth: 1),
                               to: t, destinationSlice: 0, destinationLevel: 0,
                               destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0))
                        e.endEncoding(); try finish(cb, label + "-upload")
                        try need(Data(bytes: upload.contents(), count: length) == padded, "padded upload buffer changed")
                        obs["upload_padding_preserved"] = true
                    } else {
                        seedData.withUnsafeBytes {
                            t.replace(region: MTLRegionMake2D(0, 0, c.width, c.height), mipmapLevel: 0,
                                      withBytes: $0.baseAddress!, bytesPerRow: c.width)
                        }
                        obs["upload"] = ["method": "independent-shared-replace", "bytes_per_texel": 1,
                                         "bytes_per_row": c.width, "sha256": sha(seedData)]
                    }

                    if serial == 1 {
                        let negative = try capture(t, uv, 4, 4, 0xf000, 2, "negative-skip-row-2")
                        report["negative_control"] = ["skipped_row": 2,
                            "outputs": try negative.archive("negative", store), "detected": false]
                        var rejected = false
                        do { try negative.requireClean() } catch { rejected = true }
                        try need(rejected, "negative control was accepted")
                        for p in negative.planes {
                            let health = p.health()
                            try need(health.poisonWords == 4 && health.poisonRows == [2] && health.badGuardBytes == 0 &&
                                     health.markerRows == (p.markerTag == nil ? [] : [2]),
                                     "negative control did not isolate the intentionally unwritten row")
                        }
                        negativeComplete = true
                        var info = report["negative_control"] as! [String: Any]
                        info["detected"] = true; info["ordinary_validator_rejected"] = true
                        report["negative_control"] = info
                    }

                    let cap = try capture(t, uv, c.centerCount, c.rowCount, serial * 2, UInt32.max, label)
                    obs["outputs"] = try cap.archive(label, store)
                    try cap.requireClean()
                    try need(Data(bytes: uv.contents(), count: uvData.count) == uvData, "coordinate buffer changed")
                    obs["coordinates_uploaded_bit_exact"] = true
                    obs["checks"] = try semanticChecks(cap, seed, format == .a8Unorm)
                    obs["status"] = "OBSERVED"
                } catch {
                    obs["error"] = errorInfo(error)
                    errors.append(["name": label, "error": errorInfo(error)])
                }
                observations.append(obs)
            }
        }
    }
    try need(observations.count == 10 && errors.isEmpty && negativeComplete,
             "one or more required observations or negative controls failed")
    report["status"] = "OBSERVATIONS_COMPLETE"
}

guard CommandLine.arguments.count == 3 else { exit(2) }
let store = Store(URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true))
var code: Int32 = 0
do { try run(URL(fileURLWithPath: CommandLine.arguments[1]), store) }
catch { report["failure"] = errorInfo(error); code = 3 }
report["commands"] = commands
report["observations"] = observations
report["errors"] = errors
report["negative_control_complete"] = negativeComplete
report["input_sha256"] = inputs
report["output_sha256"] = store.hashes
report["raw_output_bytes"] = store.bytes
report["raw_output_files"] = store.hashes.count
report["exit_code"] = code
do {
    let json = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
    try need(json.count <= 1_048_576, "summary size budget exceeded")
    try json.write(to: store.out.appendingPathComponent("summary.json"), options: .atomic)
} catch { code = 4 }
print("measurement_exit=\(code)")
exit(code)
