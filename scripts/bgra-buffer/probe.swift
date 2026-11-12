// Minimal SDK bridge: private buffer -> BGRA render target -> full buffer blit.
// Expected pixels and all byte comparisons belong to Rust, not this bridge.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ ok: Bool, _ message: String) throws { if !ok { throw Failure(message: message) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
struct Layout: Decodable { let width, height, pitch, offset: Int }
struct Plan: Decodable {
    let schema, parent, guard_bytes, captures: Int
    let clear_rgba: [Int]
    let layouts: [Layout]
}
let header = "capture\tcase\top\twidth\theight\tpitch\toffset\tview_bytes\tparent_bytes\tformat\tusage\tstorage\tparent_storage\tparent_identity\ttype\tdepth\tmips\tslices\tsamples\thazard\tparent_hazard\tidentity_swizzle\ttexture_parent\tframebuffer_only\talignment\tload\tstore\tdraws\tvertices\tpso_bound\tblending\twrite_mask\trasterization\talpha_coverage\talpha_one\tstatus\terror"
var report: [String: Any] = ["schema": 1, "status": "UNQUALIFIED_NATIVE_FAILURE", "cpu_fallback": false]
var receipts = header + "\n"
var saved: [String: String] = [:]
guard CommandLine.arguments.count == 3 else { exit(3) }
let source = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let out = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
func read(_ url: URL, _ max: Int) throws -> Data {
    let a = try FileManager.default.attributesOfItem(atPath: url.path)
    try need(a[.type] as? FileAttributeType == .typeRegular && ((a[.size] as? NSNumber)?.intValue ?? Int.max) <= max,
             "bounded_regular_input")
    return try Data(contentsOf: url)
}
func save(_ name: String, _ data: Data) throws {
    try need(!name.contains("/") && !name.contains("\\") && saved[name] == nil && data.count <= 1_048_576,
             "output_bound_or_duplicate")
    try data.write(to: out.appendingPathComponent(name), options: .withoutOverwriting)
    saved[name] = sha(data)
}
func run() throws {
    let planBytes = try read(out.appendingPathComponent("plan.json"), 8192)
    let input = try read(out.appendingPathComponent("parent-before.bin"), 65536)
    let p = try JSONDecoder().decode(Plan.self, from: planBytes)
    let fixed = [[1,1,256,0], [5,3,256,0], [5,3,256,256], [5,3,512,256], [104,104,512,0], [104,104,512,256]]
    try need(p.schema == 1 && p.parent == 65536 && p.guard_bytes == 64 && p.captures == 13 &&
             p.clear_rgba == [17,83,149,231] && p.layouts.map { [$0.width,$0.height,$0.pitch,$0.offset] } == fixed &&
             input.count == p.parent, "exact_synthetic_plan")
    report["inputs_sha256"] = ["plan.json": sha(planBytes), "parent-before.bin": sha(input)]
    guard let dev = MTLCreateSystemDefaultDevice(), let queue = dev.makeCommandQueue() else {
        throw Failure(message: "metal_unavailable_no_substitute")
    }
    let alignment = dev.minimumLinearTextureAlignment(for: .bgra8Unorm)
    report["device"] = ["name": dev.name, "unified_memory": dev.hasUnifiedMemory,
                        "apple1": dev.supportsFamily(.apple1), "linear_alignment": alignment]
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    try need(dev.supportsFamily(.apple1) && alignment > 0 && alignment <= 256 &&
             alignment & (alignment - 1) == 0 && 256 % alignment == 0, "fixed_alignment_or_family_unsupported")
    let usage: MTLTextureUsage = [.shaderRead, .shaderWrite, .renderTarget]
    try need(MTLPixelFormat.bgra8Unorm.rawValue == 80 && usage.rawValue == 7, "sdk_enum_contract")
    let options = MTLCompileOptions()
    options.languageVersion = .version2_4
    options.fastMathEnabled = false
    let msl = try read(source.appendingPathComponent("probe.metal"), 8192)
    guard let program = String(data: msl, encoding: .utf8) else { throw Failure(message: "source_encoding") }
    let library = try dev.makeLibrary(source: program, options: options)
    let d = MTLRenderPipelineDescriptor()
    d.vertexFunction = library.makeFunction(name: "triangle")
    d.fragmentFunction = library.makeFunction(name: "paint")
    try need(d.vertexFunction != nil && d.fragmentFunction != nil, "owned_entries_missing")
    d.rasterSampleCount = 1; d.isRasterizationEnabled = true
    d.isAlphaToCoverageEnabled = false; d.isAlphaToOneEnabled = false
    let color = d.colorAttachments[0]!
    color.pixelFormat = .bgra8Unorm; color.isBlendingEnabled = false; color.writeMask = .all
    d.depthAttachmentPixelFormat = .invalid; d.stencilAttachmentPixelFormat = .invalid
    let pipeline = try dev.makeRenderPipelineState(descriptor: d)
    report["compiler"] = ["language": options.languageVersion.rawValue, "fast_math": options.fastMathEnabled,
                          "source_sha256": sha(msl), "libraries": 1, "pipelines": 1]
    func buffer(_ size: Int, _ mode: MTLResourceOptions) throws -> MTLBuffer {
        guard let b = dev.makeBuffer(length: size, options: [mode, .hazardTrackingModeTracked]) else {
            throw Failure(message: "buffer_allocation")
        }
        try need(b.length == size && b.hazardTrackingMode == .tracked, "buffer_getters")
        return b
    }
    func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
    let guardedBytes = p.parent + 2*p.guard_bytes
    let upload = try buffer(guardedBytes, .storageModeShared)
    try need(upload.storageMode == .shared, "upload_not_shared")
    memset(upload.contents(), 0xcd, guardedBytes)
    input.withUnsafeBytes { upload.contents().advanced(by: p.guard_bytes).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let uploadBefore = snapshot(upload)
    var parents: [MTLBuffer] = [], textures: [MTLTexture] = [], downloads: [MTLBuffer] = []
    func caseIndex(_ index: Int) -> Int { index == 12 ? 2 : index / 2 }
    for i in 0..<p.captures {
        let c = p.layouts[caseIndex(i)]
        try need(c.width*4 <= c.pitch && c.pitch % 256 == 0 && c.offset % 256 == 0 &&
                 c.offset + c.pitch*c.height <= p.parent, "view_bounds")
        let parent = try buffer(p.parent, .storageModePrivate)
        try need(parent.storageMode == .private, "parent_not_private")
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .bgra8Unorm, width: c.width, height: c.height, mipmapped: false)
        td.storageMode = .private; td.usage = usage; td.hazardTrackingMode = .tracked
        td.swizzle = MTLTextureSwizzleChannels(red: .red, green: .green, blue: .blue, alpha: .alpha)
        guard let t = parent.makeTexture(descriptor: td, offset: c.offset, bytesPerRow: c.pitch) else {
            throw Failure(message: "buffer_render_target_unsupported_no_substitute")
        }
        try need(t.buffer === parent && t.bufferOffset == c.offset && t.bufferBytesPerRow == c.pitch &&
                 t.storageMode == .private && t.pixelFormat == .bgra8Unorm && t.usage == usage &&
                 t.width == c.width && t.height == c.height && t.parent == nil, "backing_authority_or_remap")
        let download = try buffer(guardedBytes, .storageModeShared)
        try need(download.storageMode == .shared, "download_not_shared")
        memset(download.contents(), 0xcd, guardedBytes)
        parents.append(parent); textures.append(t); downloads.append(download)
    }
    guard let cb = queue.makeCommandBuffer(), let initBlit = cb.makeBlitCommandEncoder() else {
        throw Failure(message: "command_or_initial_blit")
    }
    for parent in parents {
        initBlit.copy(from: upload, sourceOffset: p.guard_bytes, to: parent, destinationOffset: 0, size: p.parent)
    }
    initBlit.endEncoding()
    var rows: [[String]] = []
    for i in 0..<p.captures {
        let c = p.layouts[caseIndex(i)], t = textures[i], parent = parents[i]
        let clear = i < 12 && i % 2 == 0, draw = i < 12 && i % 2 == 1
        let op = clear ? "clear" : (draw ? "draw" : "skip")
        let pass = MTLRenderPassDescriptor()
        let a = pass.colorAttachments[0]!
        a.texture = t; a.loadAction = clear ? .clear : .load; a.storeAction = .store
        a.clearColor = MTLClearColor(red: 17.0/255.0, green: 83.0/255.0, blue: 149.0/255.0, alpha: 231.0/255.0)
        guard let e = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(message: "render_encoder") }
        if draw {
            e.setRenderPipelineState(pipeline); e.setCullMode(.none); e.setTriangleFillMode(.fill)
            e.setViewport(MTLViewport(originX: 0, originY: 0, width: Double(c.width), height: Double(c.height), znear: 0, zfar: 1))
            e.setScissorRect(MTLScissorRect(x: 0, y: 0, width: c.width, height: c.height))
            var key = UInt32(caseIndex(i))
            e.setFragmentBytes(&key, length: MemoryLayout<UInt32>.size, index: 0)
            e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
        }
        e.endEncoding()
        let sw = t.swizzle
        let identity = sw.red == .red && sw.green == .green && sw.blue == .blue && sw.alpha == .alpha
        let values: [Int] = [t.width, t.height, t.bufferBytesPerRow, t.bufferOffset, c.pitch*c.height,
            parent.length, Int(t.pixelFormat.rawValue), Int(t.usage.rawValue), Int(t.storageMode.rawValue),
            Int(parent.storageMode.rawValue), t.buffer === parent ? 1 : 0, Int(t.textureType.rawValue), t.depth,
            t.mipmapLevelCount, t.arrayLength, t.sampleCount, Int(t.hazardTrackingMode.rawValue),
            Int(parent.hazardTrackingMode.rawValue), identity ? 1 : 0, t.parent == nil ? 0 : 1,
            t.isFramebufferOnly ? 1 : 0, alignment, Int(a.loadAction.rawValue), Int(a.storeAction.rawValue),
            draw ? 1 : 0, draw ? 3 : 0, draw ? 1 : 0, color.isBlendingEnabled ? 1 : 0,
            Int(color.writeMask.rawValue), d.isRasterizationEnabled ? 1 : 0, d.isAlphaToCoverageEnabled ? 1 : 0,
            d.isAlphaToOneEnabled ? 1 : 0]
        rows.append([String(i), String(caseIndex(i)), op] + values.map(String.init))
    }
    guard let down = cb.makeBlitCommandEncoder() else { throw Failure(message: "full_parent_readback_blit") }
    for i in 0..<p.captures {
        down.copy(from: parents[i], sourceOffset: 0, to: downloads[i], destinationOffset: p.guard_bytes, size: p.parent)
    }
    down.endEncoding()
    let done = DispatchSemaphore(value: 0)
    cb.addCompletedHandler { _ in done.signal() }; cb.commit()
    try need(done.wait(timeout: .now() + 30) == .success, "gpu_timeout_no_retry")
    report["command"] = ["count": 1, "status": cb.status.rawValue, "error_present": cb.error != nil,
                         "gpu_start": cb.gpuStartTime, "gpu_end": cb.gpuEndTime, "render_passes": 13,
                         "draws": 6, "blit_copies": 26, "sampling": 0, "compute_dispatches": 0]
    for row in rows { receipts += (row + [String(cb.status.rawValue), cb.error == nil ? "0" : "1"]).joined(separator: "\t") + "\n" }
    try need(cb.status == .completed && cb.error == nil, "gpu_failed_no_fallback")
    try need(snapshot(upload) == uploadBefore, "upload_changed")
    var raw = Data()
    for b in downloads { raw.append(snapshot(b)) }
    try save("parents-after.bin", raw)
    try save("upload-after.bin", snapshot(upload))
    try save("complete.txt", Data("schema=1\ncommands=1\ncaptures=13\nclears=6\ndraws=6\nskipped_draws=1\nblit_copies=26\nsampling=0\ncompute=0\n".utf8))
    report["status"] = "CAPTURED_PENDING_RUST_VALIDATION"
    report["scope"] = "Private BGRA buffer render storage only; descriptor usage 7 does not test sampling or compute permissions."
}
var code: Int32 = 0
do { try run() }
catch {
    if let e = error as? Failure { report["reason"] = e.message }
    else { let e = error as NSError; report["reason"] = "native_api_error"; report["error_domain"] = e.domain; report["error_code"] = e.code }
    code = 3
}
do {
    try save("receipts.tsv", Data(receipts.utf8))
    report["raw_sha256"] = saved; report["exit_code"] = code
    try save("gpu.json", JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]))
} catch { code = 3 }
print("native_capture_exit=\(code)")
exit(code)
