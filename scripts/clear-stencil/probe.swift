// Minimal SDK bridge: private buffer -> R8 render target, clear-only passes with
// optional discarded Stencil8 and inactive RG16Float color1 -> full buffer blit.
// No shader source, library, pipeline or draw. Expected bytes belong to Rust.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ ok: Bool, _ message: String) throws { if !ok { throw Failure(message: message) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
struct Layout: Decodable { let width, height, pitch, offset: Int }
struct Plan: Decodable {
    let schema, parent, guard_bytes, captures, variants, negative_layout, clear_r8, clear_stencil: Int
    let layouts: [Layout]
}
let header = "capture\tlayout\tvariant\top\twidth\theight\tpitch\toffset\tview_bytes\tparent_bytes\tformat\tusage\tstorage\tparent_storage\tparent_identity\ttype\tdepth\tmips\tslices\tsamples\thazard\tparent_hazard\tidentity_swizzle\ttexture_parent\tframebuffer_only\talignment\tload\tstore\tclear_r\tstencil\tstencil_format\tstencil_width\tstencil_height\tstencil_storage\tstencil_usage\tstencil_load\tstencil_store\tcolor1\tcolor1_format\tcolor1_width\tcolor1_height\tcolor1_storage\tcolor1_usage\tcolor1_load\tcolor1_store\tdraws\tpipelines\tlibraries\tstatus\terror"
var report: [String: Any] = ["schema": 1, "status": "UNQUALIFIED_NATIVE_FAILURE", "cpu_fallback": false]
var receipts = header + "\n"
var saved: [String: String] = [:]
guard CommandLine.arguments.count == 2 else { exit(3) }
let out = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
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
    let fixed = [[1,1,256,0], [5,3,256,256], [104,104,256,0], [104,104,256,256]]
    try need(p.schema == 1 && p.parent == 65536 && p.guard_bytes == 64 && p.captures == 13 && p.variants == 3 &&
             p.negative_layout == 1 && p.clear_r8 == 0 && p.clear_stencil == 90 &&
             p.layouts.map { [$0.width,$0.height,$0.pitch,$0.offset] } == fixed &&
             input.count == p.parent, "exact_synthetic_plan")
    report["inputs_sha256"] = ["plan.json": sha(planBytes), "parent-before.bin": sha(input)]
    guard let dev = MTLCreateSystemDefaultDevice(), let queue = dev.makeCommandQueue() else {
        throw Failure(message: "metal_unavailable_no_substitute")
    }
    let alignment = dev.minimumLinearTextureAlignment(for: .r8Unorm)
    report["device"] = ["name": dev.name, "unified_memory": dev.hasUnifiedMemory,
                        "apple1": dev.supportsFamily(.apple1), "linear_alignment": alignment]
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    try need(dev.supportsFamily(.apple1) && alignment > 0 && alignment <= 256 &&
             alignment & (alignment - 1) == 0 && 256 % alignment == 0, "fixed_alignment_or_family_unsupported")
    let usage: MTLTextureUsage = [.shaderRead, .shaderWrite, .renderTarget, .pixelFormatView]
    try need(MTLPixelFormat.r8Unorm.rawValue == 10 && MTLPixelFormat.stencil8.rawValue == 253 &&
             MTLPixelFormat.rg16Float.rawValue == 65 && usage.rawValue == 23 &&
             MTLTextureUsage.renderTarget.rawValue == 4, "sdk_enum_contract")
    func buffer(_ size: Int, _ mode: MTLResourceOptions) throws -> MTLBuffer {
        guard let b = dev.makeBuffer(length: size, options: [mode, .hazardTrackingModeTracked]) else {
            throw Failure(message: "buffer_allocation")
        }
        try need(b.length == size && b.hazardTrackingMode == .tracked, "buffer_getters")
        return b
    }
    func attachment(_ format: MTLPixelFormat, _ c: Layout) throws -> MTLTexture {
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: format, width: c.width, height: c.height, mipmapped: false)
        td.storageMode = .private; td.usage = .renderTarget; td.hazardTrackingMode = .tracked
        guard let t = dev.makeTexture(descriptor: td) else { throw Failure(message: "attachment_unsupported_no_substitute") }
        try need(t.pixelFormat == format && t.width == c.width && t.height == c.height && t.storageMode == .private &&
                 t.usage == .renderTarget && t.sampleCount == 1 && t.mipmapLevelCount == 1, "attachment_getters")
        return t
    }
    func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
    let guardedBytes = p.parent + 2*p.guard_bytes
    let upload = try buffer(guardedBytes, .storageModeShared)
    try need(upload.storageMode == .shared, "upload_not_shared")
    memset(upload.contents(), 0xcd, guardedBytes)
    input.withUnsafeBytes { upload.contents().advanced(by: p.guard_bytes).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let uploadBefore = snapshot(upload)
    var parents: [MTLBuffer] = [], textures: [MTLTexture] = [], downloads: [MTLBuffer] = []
    var stencils: [MTLTexture?] = [], color1s: [MTLTexture?] = []
    func layoutIndex(_ i: Int) -> Int { i == 12 ? p.negative_layout : i / p.variants }
    func variant(_ i: Int) -> Int { i == 12 ? 0 : i % p.variants }
    for i in 0..<p.captures {
        let c = p.layouts[layoutIndex(i)]
        try need(c.width <= c.pitch && c.pitch % 256 == 0 && c.offset % 256 == 0 &&
                 c.offset + c.pitch*c.height <= p.parent, "view_bounds")
        let parent = try buffer(p.parent, .storageModePrivate)
        try need(parent.storageMode == .private, "parent_not_private")
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .r8Unorm, width: c.width, height: c.height, mipmapped: false)
        td.storageMode = .private; td.usage = usage; td.hazardTrackingMode = .tracked
        td.swizzle = MTLTextureSwizzleChannels(red: .red, green: .green, blue: .blue, alpha: .alpha)
        guard let t = parent.makeTexture(descriptor: td, offset: c.offset, bytesPerRow: c.pitch) else {
            throw Failure(message: "buffer_render_target_unsupported_no_substitute")
        }
        try need(t.buffer === parent && t.bufferOffset == c.offset && t.bufferBytesPerRow == c.pitch &&
                 t.storageMode == .private && t.pixelFormat == .r8Unorm && t.usage == usage &&
                 t.width == c.width && t.height == c.height && t.parent == nil, "backing_authority_or_remap")
        let extra = variant(i) >= 1
        if extra {
            stencils.append(try attachment(.stencil8, c)); color1s.append(try attachment(.rg16Float, c))
        } else {
            stencils.append(nil); color1s.append(nil)
        }
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
        let c = p.layouts[layoutIndex(i)], t = textures[i], parent = parents[i], v = variant(i)
        let clear = i < 12
        let pass = MTLRenderPassDescriptor()
        let a = pass.colorAttachments[0]!
        a.texture = t; a.loadAction = clear ? .clear : .load; a.storeAction = .store
        a.clearColor = MTLClearColor(red: Double(p.clear_r8), green: 0, blue: 0, alpha: 0)
        let s = pass.stencilAttachment!
        let c1 = pass.colorAttachments[1]!
        if let st = stencils[i], let ct = color1s[i] {
            s.texture = st; s.loadAction = v == 2 ? .clear : .dontCare; s.storeAction = .dontCare
            s.clearStencil = UInt32(p.clear_stencil)
            c1.texture = ct; c1.loadAction = .dontCare; c1.storeAction = .dontCare
        }
        try need((s.texture == nil) == (v == 0) && (c1.texture == nil) == (v == 0) &&
                 pass.depthAttachment.texture == nil, "attachment_variant")
        // Clear-only: the encoder is ended immediately; no pipeline, binding or draw exists.
        guard let e = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(message: "render_encoder") }
        e.endEncoding()
        let sw = t.swizzle
        let identity = sw.red == .red && sw.green == .green && sw.blue == .blue && sw.alpha == .alpha
        func att(_ x: MTLTexture?, _ load: MTLLoadAction, _ store: MTLStoreAction) -> [Int] {
            guard let x = x else { return [0, 0, 0, 0, 0, 0, 0, 0] }
            return [1, Int(x.pixelFormat.rawValue), x.width, x.height, Int(x.storageMode.rawValue),
                    Int(x.usage.rawValue), Int(load.rawValue), Int(store.rawValue)]
        }
        var values: [Int] = [t.width, t.height, t.bufferBytesPerRow, t.bufferOffset, c.pitch*c.height,
            parent.length, Int(t.pixelFormat.rawValue), Int(t.usage.rawValue), Int(t.storageMode.rawValue),
            Int(parent.storageMode.rawValue), t.buffer === parent ? 1 : 0, Int(t.textureType.rawValue), t.depth,
            t.mipmapLevelCount, t.arrayLength, t.sampleCount, Int(t.hazardTrackingMode.rawValue),
            Int(parent.hazardTrackingMode.rawValue), identity ? 1 : 0, t.parent == nil ? 0 : 1,
            t.isFramebufferOnly ? 1 : 0, alignment, Int(a.loadAction.rawValue), Int(a.storeAction.rawValue),
            Int((a.clearColor.red * 255.0).rounded())]
        values += att(s.texture, s.loadAction, s.storeAction)
        values += att(c1.texture, c1.loadAction, c1.storeAction)
        values += [0, 0, 0]
        rows.append([String(i), String(layoutIndex(i)), String(v), clear ? "clear" : "skip"] + values.map(String.init))
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
                         "draws": 0, "pipelines": 0, "libraries": 0, "blit_copies": 26, "sampling": 0,
                         "compute_dispatches": 0]
    for row in rows { receipts += (row + [String(cb.status.rawValue), cb.error == nil ? "0" : "1"]).joined(separator: "\t") + "\n" }
    try need(cb.status == .completed && cb.error == nil, "gpu_failed_no_fallback")
    try need(snapshot(upload) == uploadBefore, "upload_changed")
    var raw = Data()
    for b in downloads { raw.append(snapshot(b)) }
    try save("parents-after.bin", raw)
    try save("upload-after.bin", snapshot(upload))
    try save("complete.txt", Data("schema=1\ncommands=1\ncaptures=13\nrender_passes=13\nclears=12\nskipped_clears=1\nstencil_attachments=8\ncolor1_attachments=8\ndraws=0\npipelines=0\nlibraries=0\nblit_copies=26\nsampling=0\ncompute=0\n".utf8))
    report["status"] = "CAPTURED_PENDING_RUST_VALIDATION"
    report["scope"] = "Clear-only R8 buffer render storage with discarded Stencil8/RG16Float attachments; discarded contents are undefined and not observed."
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