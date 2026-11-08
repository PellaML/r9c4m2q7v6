// Public Metal API only. CPU code submits descriptors and checks GPU bytes;
// it never fabricates a native colour/stencil result or substitutes a renderer.
import Foundation
import Metal
import CryptoKit
import Darwin

struct Failure: Error {
    let reason: String
    var code: Int32 = 3
}
func need(_ value: Bool, _ reason: String) throws {
    if !value { throw Failure(reason: reason) }
}
func sha(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}
func errorInfo(_ error: Error) -> [String: Any] {
    if let e = error as? Failure { return ["reason": e.reason] }
    let e = error as NSError
    // Native error descriptions/userInfo can contain paths; do not serialize them.
    return ["reason": "native_api_error", "domain": e.domain, "code": e.code]
}
func word(_ bytes: [UInt8], _ at: Int) -> UInt32 {
    UInt32(bytes[at]) | UInt32(bytes[at + 1]) << 8 |
    UInt32(bytes[at + 2]) << 16 | UInt32(bytes[at + 3]) << 24
}

struct Face: Codable {
    var compare: String
    var fail: String
    var depthFail: String
    var pass: String
    var readMask: UInt32
    var writeMask: UInt32
}
struct Draw: Codable {
    var mode: String
    var front: Face
    var back: Face
    var frontRef: UInt32
    var backRef: UInt32
    var splitRef: Bool
    var rect: [Int]
    var reverse: Bool
    var winding: String
    var color: [UInt8]
    var colorWrite: Bool
    var discard: String
}
struct CaseSpec: Codable {
    var name: String
    var topic: String
    var load: String
    var store: String
    var clear: UInt32
    var draws: [Draw]
    var anchorColor: [UInt8]?
    var anchorStencil: UInt8?
}
struct Manifest: Decodable {
    let schema: Int
    let caseCount: Int
    let width: Int
    let height: Int
    let vertexFile: String
    let colorFormat: String
    let stencilFormat: String
    let cases: [CaseSpec]
}

var report: [String: Any] = ["schema": 1, "status": "FAILED_NOT_PASS"]
var commands: [[String: Any]] = []
var observations: [[String: Any]] = []

final class Store {
    let root: URL
    let reportName: String
    var hashes: [String: String] = [:]
    var bytes = 0
    init(_ root: URL, _ mode: String) {
        self.root = root
        reportName = mode == "blit-control" ? "blit.json" : "native.json"
    }
    func save(_ name: String, _ data: Data) throws -> String {
        try need(name.range(of: "^[a-z0-9][a-z0-9.-]*\\.bin$", options: .regularExpression) != nil,
                 "unsafe_output_name")
        try need(hashes[name] == nil && hashes.count < 224, "output_file_budget")
        try need(data.count <= 1_048_576 && bytes + data.count <= 6_291_456, "output_byte_budget")
        try data.write(to: root.appendingPathComponent(name), options: .withoutOverwriting)
        hashes[name] = sha(data); bytes += data.count
        return name
    }
    func checkpoint() throws {
        report["commands"] = commands
        report["observations"] = observations
        report["output_sha256"] = hashes
        report["raw_output_files"] = hashes.count
        report["raw_output_bytes"] = bytes
        let d = try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys, .prettyPrinted])
        try need(d.count <= 1_048_576, "native_report_byte_budget")
        try d.write(to: root.appendingPathComponent(reportName), options: .atomic)
    }
}

// Every file retains its prefix, row padding and suffix. Payload extraction is
// a lossless byte selection, not a conversion, screenshot, or CPU result path.
final class Readback {
    let buffer: MTLBuffer
    let width: Int
    let height: Int
    let bpp: Int
    let pitch: Int
    let offset = 256
    let suffix = 256
    init(_ device: MTLDevice, _ width: Int, _ height: Int, _ bpp: Int) throws {
        self.width = width; self.height = height; self.bpp = bpp
        pitch = ((width * bpp + 255) / 256) * 256
        guard let b = device.makeBuffer(length: 256 + pitch * height + 256,
                                       options: .storageModeShared) else {
            throw Failure(reason: "readback_allocation")
        }
        buffer = b
        memset(b.contents(), 0xcd, b.length)
        try need(b.storageMode == .shared, "readback_storage_mismatch")
    }
    func raw() -> Data { Data(bytes: buffer.contents(), count: buffer.length) }
    func payload() -> [UInt8] {
        let bytes = [UInt8](raw())
        var result: [UInt8] = []
        for y in 0..<height {
            result.append(contentsOf: bytes[(offset + y * pitch)..<(offset + y * pitch + width * bpp)])
        }
        return result
    }
    func badGuards() -> Int {
        let bytes = [UInt8](raw())
        var bad = 0
        for i in bytes.indices {
            let local = i - offset
            let payload = local >= 0 && local < pitch * height && local % pitch < width * bpp
            if !payload && bytes[i] != 0xcd { bad += 1 }
        }
        return bad
    }
    func archive(_ name: String, _ store: Store) throws -> [String: Any] {
        ["file": try store.save(name, raw()), "width": width, "height": height,
         "bytes_per_pixel": bpp, "offset": offset, "bytes_per_row": pitch,
         "bytes_per_image": pitch * height, "suffix_bytes": suffix,
         "length": buffer.length, "poison_guard_byte": 205,
         "bad_guard_bytes": badGuards(), "storage": buffer.storageMode.rawValue,
         "storage_name": "shared", "cpu_cache_mode": buffer.cpuCacheMode.rawValue,
         "hazard_tracking_mode": buffer.hazardTrackingMode.rawValue]
    }
}

struct Capture {
    let color: Readback
    let query: Readback?
    let stencil: Readback?
    let marks: Readback
    let tag: UInt32

    func errors(_ spec: CaseSpec?, ramp: Bool = false) -> [String] {
        var e: [String] = []
        for (name, p) in [("color", Optional(color)), ("query", query),
                          ("stencil", stencil), ("markers", Optional(marks))] {
            if let p = p, p.badGuards() != 0 { e.append("guard-" + name) }
        }
        let m = marks.payload()
        for phase in 0..<4 {
            let i = phase * 16
            if word(m, i) != 0x53543847 || word(m, i + 4) != UInt32(phase) ||
               word(m, i + 8) != tag || word(m, i + 12) != ~UInt32(phase) {
                e.append("marker-phase-\(phase)")
            }
        }
        let c = color.payload()
        for i in stride(from: 0, to: c.count, by: 4) {
            if c[i + 3] != 255 || c[i..<i+4].allSatisfy({ $0 == 0xcd }) {
                e.append("color-unwritten-or-invalid-alpha"); break
            }
        }
        var measured: [UInt8]? = nil
        if let q = query {
            let p = q.payload()
            var values: [UInt8] = []
            for i in stride(from: 0, to: p.count, by: 4) {
                if p[i + 1] != 255 - p[i] || p[i + 2] != 0x53 || p[i + 3] != 255 {
                    e.append("query-unwritten-or-invalid-signature"); break
                }
                values.append(p[i])
            }
            if values.count == q.width * q.height { measured = values }
        }
        if let s = stencil {
            let values = s.payload()
            if let q = measured, q != values { e.append("direct-query-disagreement") }
            if measured == nil && query == nil { measured = values }
            // 0xcd is a legitimate Stencil8 value, including in the ramp. Never
            // reject it as poison in isolation; ramp/query agreement authenticates it.
        }
        if ramp {
            if measured != (0...255).map({ UInt8($0) }) { e.append("ramp-stencil-anchor") }
            if c != Array(repeating: [UInt8](arrayLiteral: 0, 0, 0, 255), count: 256).flatMap({ $0 }) {
                e.append("ramp-color-anchor")
            }
        }
        if let s = spec {
            if let expected = s.anchorColor {
                if c != Array(repeating: expected, count: color.width * color.height).flatMap({ $0 }) {
                    e.append("anchor-color")
                }
            }
            if let expected = s.anchorStencil {
                if measured != Array(repeating: expected, count: color.width * color.height) {
                    e.append("anchor-stencil")
                }
            }
        }
        return e
    }
    func archive(_ prefix: String, _ store: Store) throws -> [String: Any] {
        var o: [String: Any] = ["color": try color.archive(prefix + ".color.bin", store),
                                "markers": try marks.archive(prefix + ".markers.bin", store)]
        if let q = query { o["query"] = try q.archive(prefix + ".query.bin", store) }
        if let s = stencil { o["stencil"] = try s.archive(prefix + ".stencil.bin", store) }
        return o
    }
}

func compare(_ name: String) throws -> MTLCompareFunction {
    switch name {
    case "always": return .always
    case "notEqual": return .notEqual
    case "equal": return .equal // Internal GPU readout only, never a test substitute.
    default: throw Failure(reason: "invalid_compare")
    }
}
func operation(_ name: String) throws -> MTLStencilOperation {
    switch name {
    case "keep": return .keep
    case "zero": return .zero
    case "replace": return .replace
    default: throw Failure(reason: "invalid_operation")
    }
}
func stencilDescriptor(_ face: Face) throws -> MTLStencilDescriptor {
    let d = MTLStencilDescriptor()
    d.stencilCompareFunction = try compare(face.compare)
    d.stencilFailureOperation = try operation(face.fail)
    d.depthFailureOperation = try operation(face.depthFail)
    d.depthStencilPassOperation = try operation(face.pass)
    // Intentionally full-width public API parameters; no UInt8 conversion here.
    d.readMask = face.readMask; d.writeMask = face.writeMask
    return d
}
func faceInfo(_ face: MTLStencilDescriptor?) -> Any {
    guard let d = face else { return NSNull() }
    return ["compare": d.stencilCompareFunction.rawValue,
            "fail": d.stencilFailureOperation.rawValue,
            "depth_fail": d.depthFailureOperation.rawValue,
            "pass": d.depthStencilPassOperation.rawValue,
            "read_mask_u32": d.readMask, "write_mask_u32": d.writeMask] as [String: Any]
}
func depthInfo(_ d: MTLDepthStencilDescriptor) -> [String: Any] {
    ["depth_compare": d.depthCompareFunction.rawValue, "depth_write": d.isDepthWriteEnabled,
     "front": faceInfo(d.frontFaceStencil), "back": faceInfo(d.backFaceStencil)]
}
func plainFace(_ op: String = "replace", _ cmp: String = "always") -> Face {
    Face(compare: cmp, fail: "keep", depthFail: "keep", pass: op,
         readMask: UInt32.max, writeMask: UInt32.max)
}
func seedDraw(_ reference: UInt32, _ size: Int) -> Draw {
    Draw(mode: "enabled", front: plainFace(), back: plainFace(),
         frontRef: reference, backRef: reference, splitRef: false,
         rect: [0, 0, size, size], reverse: false, winding: "ccw",
         color: [255, 0, 0, 255], colorWrite: false, discard: "none")
}

final class Harness {
    let device: MTLDevice
    let queue: MTLCommandQueue
    let vertices: MTLBuffer
    let inputVertices: Data
    let paintAll: MTLRenderPipelineState
    let paintNone: MTLRenderPipelineState
    let queryPSO: MTLRenderPipelineState
    let markerPSO: MTLComputePipelineState
    let queryState: MTLDepthStencilState
    let store: Store
    var commandCount = 0
    var drawCount = 0
    var stateCache: [String: MTLDepthStencilState] = [:]

    init(_ source: URL, _ store: Store) throws {
        self.store = store
        guard let dev = MTLCreateSystemDefaultDevice(), let q = dev.makeCommandQueue() else {
            throw Failure(reason: "no_metal_device_or_queue")
        }
        device = dev; queue = q
        report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
        report["toolchain_receipt"] = "../run-receipt.json"
        var families: [String: Bool] = [:]
        let knownFamilies: [(String, MTLGPUFamily)] = [
            ("apple1", .apple1), ("apple7", .apple7), ("mac2", .mac2), ("common3", .common3)]
        for (name, family) in knownFamilies {
            families[name] = dev.supportsFamily(family)
        }
        if #available(macOS 13.0, *) {
            families["apple8"] = dev.supportsFamily(.apple8)
            families["metal3"] = dev.supportsFamily(.metal3)
        }
        if #available(macOS 14.0, *) { families["apple9"] = dev.supportsFamily(.apple9) }
        var deviceInfo: [String: Any] = ["name": dev.name, "registry_id": String(dev.registryID),
            "has_unified_memory": dev.hasUnifiedMemory, "is_low_power": dev.isLowPower,
            "is_removable": dev.isRemovable, "families": families, "family_queries_exhaustive": false]
        if #available(macOS 14.0, *) { deviceInfo["architecture"] = dev.architecture.name }
        report["device"] = deviceInfo
        report["contract"] = ["cpu_renderer": false, "tolerance": 0,
            "color_format": "rgba8Uint", "stencil_format": "stencil8", "depth_attachment": "nil",
            "shader_depth_output": false, "shader_stencil_output": false,
            "early_fragment_tests_requested": false, "samples": 1,
            "stencil_storage": "private", "color_storage": "private", "readback_storage": "shared",
            "readout": "GPU Equal reference sweep 0..255; direct standalone Stencil8 blit when separately proven",
            "dontcare_store": "no later stencil load, query, copy, or stencil output file"] as [String: Any]
        report["marker_layout"] = "little-endian uint4 per phase: 0x53543847, phase, case tag, ~phase"
        report["phase_order"] = ["before test render", "after render and colour/direct copies",
                                  "after readout or explicit no-readout", "capture complete"]
        let code = try Data(contentsOf: source)
        inputVertices = try Data(contentsOf: store.root.appendingPathComponent("vertices.bin"))
        try need(code.count < 65536 && inputVertices.count == 96, "shader_or_vertex_input_bound")
        report["shader_sha256"] = sha(code)
        report["vertex_sha256"] = sha(inputVertices)
        vertices = try inputVertices.withUnsafeBytes { bytes throws -> MTLBuffer in
            guard let p = bytes.baseAddress,
                  let b = dev.makeBuffer(bytes: p, length: bytes.count, options: .storageModeShared) else {
                throw Failure(reason: "vertex_buffer_allocation")
            }
            return b
        }
        let options = MTLCompileOptions()
        options.fastMathEnabled = false
        report["msl_compile"] = ["fast_math": options.fastMathEnabled,
                                 "language_version": options.languageVersion.rawValue,
                                 "language_selection": "SDK default"]
        guard let text = String(data: code, encoding: .utf8) else { throw Failure(reason: "shader_utf8") }
        let library = try dev.makeLibrary(source: text, options: options)
        func pipeline(_ fragment: String, _ write: Bool) throws -> MTLRenderPipelineState {
            let d = MTLRenderPipelineDescriptor()
            d.vertexFunction = library.makeFunction(name: "screen_triangle")
            d.fragmentFunction = library.makeFunction(name: fragment)
            try need(d.vertexFunction != nil && d.fragmentFunction != nil, "render_function_missing")
            d.colorAttachments[0].pixelFormat = .rgba8Uint
            d.colorAttachments[0].writeMask = write ? .all : MTLColorWriteMask(rawValue: 0)
            d.colorAttachments[0].isBlendingEnabled = false
            d.depthAttachmentPixelFormat = .invalid
            d.stencilAttachmentPixelFormat = .stencil8
            d.rasterSampleCount = 1
            return try dev.makeRenderPipelineState(descriptor: d)
        }
        paintAll = try pipeline("paint", true)
        paintNone = try pipeline("paint", false)
        queryPSO = try pipeline("query_stencil", true)
        guard let marker = library.makeFunction(name: "mark_phase") else {
            throw Failure(reason: "marker_function_missing")
        }
        markerPSO = try dev.makeComputePipelineState(function: marker)
        let d = MTLDepthStencilDescriptor()
        d.depthCompareFunction = .always; d.isDepthWriteEnabled = false
        var f = plainFace("keep", "equal"); f.readMask = 255; f.writeMask = 0
        d.frontFaceStencil = try stencilDescriptor(f); d.backFaceStencil = try stencilDescriptor(f)
        guard let state = dev.makeDepthStencilState(descriptor: d) else {
            throw Failure(reason: "query_state_allocation")
        }
        queryState = state
        report["query_descriptor_getters"] = depthInfo(d)
        report["pipeline_submitted_descriptors"] = ["color_format": MTLPixelFormat.rgba8Uint.rawValue,
            "stencil_format": MTLPixelFormat.stencil8.rawValue,
            "depth_format": MTLPixelFormat.invalid.rawValue, "raster_samples": 1,
            "color_write_masks": [MTLColorWriteMask.all.rawValue, 0], "blending": false,
            "vertex": "screen_triangle", "fragments": ["paint", "query_stencil"]] as [String: Any]
        report["default_descriptor_getters"] = depthInfo(MTLDepthStencilDescriptor())
    }

    func texture(_ format: MTLPixelFormat, _ w: Int, _ h: Int) throws -> MTLTexture {
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: format, width: w, height: h, mipmapped: false)
        d.storageMode = .private; d.usage = .renderTarget
        d.hazardTrackingMode = .tracked
        guard let t = device.makeTexture(descriptor: d) else { throw Failure(reason: "texture_allocation") }
        try need(t.pixelFormat == format && t.storageMode == .private && t.width == w && t.height == h &&
                 t.textureType == .type2D && t.sampleCount == 1 && t.mipmapLevelCount == 1 &&
                 t.depth == 1 && t.arrayLength == 1 && t.usage == .renderTarget && !t.isFramebufferOnly,
                 "texture_properties_mismatch")
        return t
    }
    func textureInfo(_ t: MTLTexture) -> [String: Any] {
        ["pixel_format": t.pixelFormat.rawValue, "storage": t.storageMode.rawValue,
         "usage": t.usage.rawValue, "type": t.textureType.rawValue, "width": t.width,
         "height": t.height, "depth": t.depth, "array_length": t.arrayLength,
         "mips": t.mipmapLevelCount, "samples": t.sampleCount,
         "framebuffer_only": t.isFramebufferOnly, "buffer_backed": t.buffer != nil,
         "cpu_cache_mode": t.cpuCacheMode.rawValue, "hazard_tracking_mode": t.hazardTrackingMode.rawValue]
    }
    func command(_ label: String) throws -> MTLCommandBuffer {
        try need(commandCount < 160, "command_buffer_count_budget")
        guard let cb = queue.makeCommandBuffer() else { throw Failure(reason: "command_allocation") }
        cb.label = label; commandCount += 1
        return cb
    }
    func finish(_ cb: MTLCommandBuffer, directAttempt: Bool = false) throws {
        cb.commit(); cb.waitUntilCompleted()
        var r: [String: Any] = ["label": cb.label ?? "unlabelled", "committed": true,
            "wait_until_completed_returned": true, "status": cb.status.rawValue,
            "completed": cb.status == .completed, "error_present": cb.error != nil,
            "gpu_start_time": cb.gpuStartTime, "gpu_end_time": cb.gpuEndTime]
        if let e = cb.error { r["error"] = errorInfo(e) }
        commands.append(r)
        try store.checkpoint()
        if cb.status != .completed || cb.error != nil {
            throw Failure(reason: directAttempt ? "direct_blit_command_failed" : "command_failed",
                          code: directAttempt ? 77 : 3)
        }
    }
    func mark(_ cb: MTLCommandBuffer, _ marks: Readback, _ tag: UInt32,
              _ first: UInt32, _ count: UInt32 = 1, skip: UInt32 = UInt32.max) throws {
        guard let e = cb.makeComputeCommandEncoder() else { throw Failure(reason: "marker_encoder") }
        e.setComputePipelineState(markerPSO)
        e.setBuffer(marks.buffer, offset: marks.offset, index: 0)
        var p = SIMD4<UInt32>(tag, first, skip, count)
        e.setBytes(&p, length: 16, index: 1)
        e.dispatchThreadgroups(MTLSize(width: Int(count), height: 1, depth: 1),
                              threadsPerThreadgroup: MTLSize(width: 1, height: 1, depth: 1))
        e.endEncoding()
    }
    func copy(_ cb: MTLCommandBuffer, _ texture: MTLTexture, _ out: Readback) throws {
        guard let e = cb.makeBlitCommandEncoder() else { throw Failure(reason: "blit_encoder") }
        // Standalone Stencil8 uses the ordinary texture-to-buffer overload. No
        // combined depth/stencil format, aspect view, or undocumented blit flags.
        e.copy(from: texture, sourceSlice: 0, sourceLevel: 0,
               sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
               sourceSize: MTLSize(width: out.width, height: out.height, depth: 1),
               to: out.buffer, destinationOffset: out.offset,
               destinationBytesPerRow: out.pitch, destinationBytesPerImage: out.pitch * out.height)
        e.endEncoding()
    }
    func pass(_ color: MTLTexture, _ stencil: MTLTexture, _ load: MTLLoadAction,
              _ action: MTLStoreAction, _ clear: UInt32, query: Bool = false) -> MTLRenderPassDescriptor {
        let d = MTLRenderPassDescriptor()
        d.colorAttachments[0].texture = color
        d.colorAttachments[0].loadAction = .clear; d.colorAttachments[0].storeAction = .store
        d.colorAttachments[0].clearColor = MTLClearColorMake(0, 0, 0, query ? 0 : 255)
        d.stencilAttachment.texture = stencil
        d.stencilAttachment.loadAction = load; d.stencilAttachment.storeAction = action
        d.stencilAttachment.clearStencil = clear
        d.depthAttachment.texture = nil
        d.depthAttachment.loadAction = .dontCare; d.depthAttachment.storeAction = .dontCare
        return d
    }
    func passInfo(_ p: MTLRenderPassDescriptor) -> [String: Any] {
        ["stencil_load": p.stencilAttachment.loadAction.rawValue,
         "stencil_store": p.stencilAttachment.storeAction.rawValue,
         "clear_stencil_u32": p.stencilAttachment.clearStencil,
         "color_load": p.colorAttachments[0].loadAction.rawValue,
         "color_store": p.colorAttachments[0].storeAction.rawValue,
         "clear_color": [p.colorAttachments[0].clearColor.red, p.colorAttachments[0].clearColor.green,
                         p.colorAttachments[0].clearColor.blue, p.colorAttachments[0].clearColor.alpha],
         "depth_texture_nil": p.depthAttachment.texture == nil]
    }
    func setup(_ e: MTLRenderCommandEncoder, _ size: Int) {
        e.setViewport(MTLViewport(originX: 0, originY: 0, width: Double(size), height: Double(size), znear: 0, zfar: 1))
        e.setCullMode(.none); e.setFrontFacing(.counterClockwise)
        e.setVertexBuffer(vertices, offset: 0, index: 0)
    }
    func draw(_ e: MTLRenderCommandEncoder, _ d: Draw, _ size: Int, skip: Bool = false) throws -> [String: Any] {
        let dd = MTLDepthStencilDescriptor()
        if d.mode == "enabled" {
            dd.depthCompareFunction = .always; dd.isDepthWriteEnabled = false
            dd.frontFaceStencil = try stencilDescriptor(d.front)
            dd.backFaceStencil = try stencilDescriptor(d.back)
        }
        let key = String(data: try JSONSerialization.data(withJSONObject: depthInfo(dd), options: .sortedKeys), encoding: .utf8)!
        if d.mode == "nil" {
            e.setDepthStencilState(nil)
        } else {
            if stateCache[key] == nil {
                guard let state = device.makeDepthStencilState(descriptor: dd) else { throw Failure(reason: "test_state_allocation") }
                stateCache[key] = state
            }
            e.setDepthStencilState(stateCache[key])
        }
        e.setRenderPipelineState(d.colorWrite ? paintAll : paintNone)
        e.setFrontFacing(d.winding == "ccw" ? .counterClockwise : .clockwise)
        e.setScissorRect(MTLScissorRect(x: d.rect[0], y: d.rect[1], width: d.rect[2], height: d.rect[3]))
        if d.splitRef {
            e.setStencilReferenceValues(front: d.frontRef, back: d.backRef)
        } else {
            e.setStencilReferenceValue(d.frontRef)
        }
        let discard: UInt32 = d.discard == "all" ? 1 : (d.discard == "left" ? 2 : 0)
        let p = [SIMD4<UInt32>(UInt32(d.color[0]), UInt32(d.color[1]), UInt32(d.color[2]), UInt32(d.color[3])),
                 SIMD4<UInt32>(discard, UInt32(size), 0, 0)]
        p.withUnsafeBufferPointer {
            e.setFragmentBytes($0.baseAddress!, length: $0.count * MemoryLayout<SIMD4<UInt32>>.stride, index: 0)
        }
        if !skip {
            try need(drawCount < 14000, "draw_count_budget")
            e.drawPrimitives(type: .triangle, vertexStart: d.reverse ? 3 : 0, vertexCount: 3)
            drawCount += 1
        }
        return ["state_mode": d.mode, "descriptor_getters": depthInfo(dd),
                "submitted_masks_u32": ["front": [d.front.readMask, d.front.writeMask],
                                         "back": [d.back.readMask, d.back.writeMask]],
                "nil_state_bound": d.mode == "nil", "front_reference_call_u32": d.frontRef,
                "back_reference_call_u32": d.backRef, "split_reference_call": d.splitRef,
                "reference_getter_available": false, "scissor": d.rect,
                "front_winding_call": d.winding, "cull_mode_call": "none",
                "vertex_start": d.reverse ? 3 : 0, "vertex_count": 3,
                "draw_submitted": !skip, "color_write_mask_call": d.colorWrite ? MTLColorWriteMask.all.rawValue : 0,
                "fragment_color_u8": d.color, "discard_mode": d.discard]
    }
    func ramp(_ e: MTLRenderCommandEncoder) throws -> [String: Any] {
        var first: [String: Any] = [:]
        for i in 0..<256 {
            var d = seedDraw(UInt32(i), 16)
            d.rect = [i % 16, i / 16, 1, 1]
            let r = try draw(e, d, 16)
            if i == 0 { first = r }
        }
        return ["first_draw_descriptor": first, "count": 256,
                "construction": "reference=i, scissor=(i%16,i/16,1,1), i=0..255; full write mask, Always Replace, colour mask zero"]
    }
    func query(_ cb: MTLCommandBuffer, _ stencil: MTLTexture, _ color: MTLTexture, _ size: Int) throws -> [String: Any] {
        let p = pass(color, stencil, .load, .store, 0, query: true)
        guard let e = cb.makeRenderCommandEncoder(descriptor: p) else { throw Failure(reason: "query_encoder") }
        setup(e, size)
        e.setScissorRect(MTLScissorRect(x: 0, y: 0, width: size, height: size))
        e.setRenderPipelineState(queryPSO); e.setDepthStencilState(queryState)
        for reference in 0..<256 {
            e.setStencilReferenceValue(UInt32(reference))
            var params = SIMD4<UInt32>(UInt32(reference), 0, 0, 0)
            e.setFragmentBytes(&params, length: 16, index: 0)
            try need(drawCount < 14000, "draw_count_budget")
            e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
            drawCount += 1
        }
        e.endEncoding()
        return ["pass_descriptor_getters": passInfo(p), "references": [0, 255], "draw_count": 256,
                "shader_rgba": "reference, 255-reference, 0x53, 255",
                "unmatched_clear_rgba": [0, 0, 0, 0], "state_descriptor": "query_descriptor_getters"]
    }

    // This is a separate process. An API-validation abort can leave only the
    // checkpoint, never a success receipt or an invented direct readback.
    func blitControl() throws {
        let color = try texture(.rgba8Uint, 16, 16), stencil = try texture(.stencil8, 16, 16)
        let c = try Readback(device, 16, 16, 4), s = try Readback(device, 16, 16, 1)
        let marks = try Readback(device, 4, 1, 16)
        let tag: UInt32 = 0x1000
        let cb = try command("direct-control-render")
        try mark(cb, marks, tag, 0)
        let p = pass(color, stencil, .clear, .store, 0)
        guard let e = cb.makeRenderCommandEncoder(descriptor: p) else { throw Failure(reason: "control_encoder") }
        setup(e, 16)
        report["ramp_seed"] = try ramp(e)
        e.endEncoding()
        try copy(cb, color, c)
        try mark(cb, marks, tag, 1)
        try finish(cb)
        report["stage"] = "before_direct_stencil_blit"
        report["status"] = "DIRECT_BLIT_NOT_YET_PROVEN"
        report["actual_textures"] = ["color": textureInfo(color), "stencil": textureInfo(stencil)]
        report["render_pass_descriptor_getters"] = passInfo(p)
        try store.checkpoint()
        let cb2 = try command("direct-control-stencil-copy")
        try copy(cb2, stencil, s)
        try mark(cb2, marks, tag, 2, 2)
        try finish(cb2, directAttempt: true)
        report["stage"] = "direct_stencil_blit_completed"
        let cap = Capture(color: c, query: nil, stencil: s, marks: marks, tag: tag)
        report["outputs"] = try cap.archive("direct-control", store)
        let errors = cap.errors(nil, ramp: true)
        report["validation_errors"] = errors
        try need(errors.isEmpty, "completed_direct_blit_failed_exact_validation")
        report["status"] = "DIRECT_BLIT_EXACT"
        report["tag"] = tag
        report["gpu_initialized_values"] = "all 256 bytes, rendered at one pixel each; not CPU-uploaded stencil"
    }

    func capture(_ spec: CaseSpec, _ tag: UInt32, direct: Bool,
                 calibration: Bool = false, mutation: String = "none") throws -> Capture {
        let size = calibration ? 16 : 8
        let publishStencil = spec.store == "store"
        let color = try texture(.rgba8Uint, size, size), stencil = try texture(.stencil8, size, size)
        let c = try Readback(device, size, size, 4)
        let q = publishStencil ? try Readback(device, size, size, 4) : nil
        let s = publishStencil && direct ? try Readback(device, size, size, 1) : nil
        let marks = try Readback(device, 4, 1, 16)
        var obs: [String: Any] = ["name": spec.name, "topic": spec.topic, "tag": tag,
            "status": "FAILED_NOT_PASS", "mutation": mutation, "calibration": calibration,
            "clear_stencil_call_u32": spec.clear,
            "actual_textures": ["color": textureInfo(color), "stencil": textureInfo(stencil)],
            "stencil_publication": publishStencil ? (direct ? "direct-and-gpu-query" : "gpu-query-only") : "NONE_DONTCARE_STORE",
            "post_pass_stencil_access": publishStencil, "source_case": calibration ? "synthetic-ramp" : spec.name]
        let cb = try command(spec.name + "-test")
        try mark(cb, marks, tag, 0)
        let p = pass(color, stencil, spec.load == "clear" ? .clear : .dontCare,
                     publishStencil ? .store : .dontCare, spec.clear)
        obs["render_pass_descriptor_getters"] = passInfo(p)
        guard let e = cb.makeRenderCommandEncoder(descriptor: p) else { throw Failure(reason: "test_encoder") }
        setup(e, size)
        if calibration { obs["ramp_seed"] = try ramp(e) }
        else {
            var receipts: [[String: Any]] = []
            for (i, d) in spec.draws.enumerated() {
                receipts.append(try draw(e, d, size, skip: mutation == "skip-draw" && i == 0))
            }
            obs["draw_calls"] = receipts
        }
        e.endEncoding()
        try copy(cb, color, c)
        if let s = s { try copy(cb, stencil, s) }
        try mark(cb, marks, tag, 1)
        try finish(cb)
        let cb2 = try command(spec.name + "-readout")
        if let q = q {
            let qt = try texture(.rgba8Uint, size, size)
            obs["query_texture"] = textureInfo(qt)
            obs["gpu_query"] = try query(cb2, stencil, qt, size)
            try copy(cb2, qt, q)
        } else {
            // No encoder in this branch binds the discarded stencil texture.
            obs["gpu_query"] = NSNull()
        }
        try mark(cb2, marks, tag, 2, 2, skip: mutation == "skip-marker" ? 3 : UInt32.max)
        try finish(cb2)
        let cap = Capture(color: c, query: q, stencil: s, marks: marks, tag: tag)
        obs["outputs"] = try cap.archive(spec.name, store)
        let errors = cap.errors(calibration ? nil : spec, ramp: calibration)
        obs["ordinary_validation_errors"] = errors
        if mutation == "none" {
            obs["status"] = errors.isEmpty ? "OBSERVED" : "FAILED_NOT_PASS"
        } else {
            let expected = mutation == "skip-draw" ? ["anchor-color", "anchor-stencil"] : ["marker-phase-3"]
            let rejected = errors == expected
            obs["expected_rejection_errors"] = expected
            obs["ordinary_validator_rejected"] = rejected
            obs["status"] = rejected ? "NEGATIVE_REJECTED" : "FAILED_NOT_PASS"
        }
        observations.append(obs)
        try store.checkpoint()
        if mutation == "none" { try need(errors.isEmpty, "ordinary_capture_rejected") }
        else {
            let expected = mutation == "skip-draw" ? ["anchor-color", "anchor-stencil"] : ["marker-phase-3"]
            try need(errors == expected, "gpu_negative_not_rejected_for_exact_expected_reason")
        }
        return cap
    }
    func observe(_ manifest: Manifest, direct: Bool) throws {
        report["direct_blit_enabled"] = direct
        report["case_manifest_count"] = manifest.caseCount
        let calibration = CaseSpec(name: "query-calibration", topic: "all-256-gpu-byte-readout-control",
            load: "clear", store: "store", clear: 0, draws: [], anchorColor: nil, anchorStencil: nil)
        _ = try capture(calibration, 1, direct: direct, calibration: true)
        for (i, c) in manifest.cases.enumerated() {
            _ = try capture(c, UInt32(i + 2), direct: direct)
        }
        guard let baseline = manifest.cases.first(where: { $0.name == "always-replace-1" }) else {
            throw Failure(reason: "negative_reference_missing")
        }
        var wrong = baseline; wrong.name = "negative-skip-draw"
        _ = try capture(wrong, 100, direct: direct, mutation: "skip-draw")
        var missed = baseline; missed.name = "negative-skip-marker"
        _ = try capture(missed, 101, direct: direct, mutation: "skip-marker")
        try need(Data(bytes: vertices.contents(), count: inputVertices.count) == inputVertices,
                 "vertex_input_buffer_changed")
        report["vertex_input_preserved"] = true
        report["negative_controls_complete"] = true
        report["command_buffer_count"] = commandCount
        report["draw_count"] = drawCount
        report["status"] = "OBSERVATIONS_COMPLETE"
        report["behavior_claim"] = "raw observations for this exact device/toolchain only; exploratory semantics are not preset answers"
    }
}

func validate(_ m: Manifest) throws {
    try need(m.schema == 1 && m.caseCount == 45 && m.cases.count == m.caseCount &&
             m.width == 8 && m.height == 8 && m.vertexFile == "vertices.bin" &&
             m.colorFormat == "rgba8Uint" && m.stencilFormat == "stencil8", "manifest_contract")
    try need(Set(m.cases.map { $0.name }).count == m.caseCount, "duplicate_case_name")
    for c in m.cases {
        try need(c.name.range(of: "^[a-z0-9][a-z0-9-]{0,63}$", options: .regularExpression) != nil &&
                 ["clear", "dontCare"].contains(c.load) && ["store", "dontCare"].contains(c.store) &&
                 c.draws.count <= 3, "case_contract")
        if let a = c.anchorColor { try need(a.count == 4, "anchor_color_shape") }
        if c.store == "dontCare" { try need(c.anchorStencil == nil, "dontcare_stencil_anchor_forbidden") }
        for d in c.draws {
            try need(["enabled", "default", "nil"].contains(d.mode) && ["ccw", "cw"].contains(d.winding) &&
                     ["none", "all", "left"].contains(d.discard) && d.color.count == 4 &&
                     d.rect.count == 4 && d.rect[0] >= 0 && d.rect[1] >= 0 &&
                     d.rect[2] > 0 && d.rect[3] > 0 && d.rect[0] + d.rect[2] <= 8 &&
                     d.rect[1] + d.rect[3] <= 8 && (d.splitRef || d.frontRef == d.backRef), "draw_contract")
            for f in [d.front, d.back] {
                try need(["always", "notEqual"].contains(f.compare), "test_compare_contract")
                _ = try operation(f.fail); _ = try operation(f.depthFail); _ = try operation(f.pass)
            }
        }
        if c.load == "dontCare" {
            guard let d = c.draws.first else { throw Failure(reason: "dontcare_not_initialized") }
            for f in [d.front, d.back] {
                try need(f.compare == "always" && f.pass == "replace" && f.writeMask == UInt32.max,
                         "dontcare_initializer_state")
            }
            try need(d.mode == "enabled" && d.rect == [0, 0, 8, 8] && d.discard == "none",
                     "dontcare_initializer_coverage")
        }
    }
}

guard CommandLine.arguments.count == 5 else { exit(2) }
let mode = CommandLine.arguments[1], readout = CommandLine.arguments[4]
guard ["blit-control", "observe"].contains(mode) && ["direct", "sweep"].contains(readout) else { exit(2) }
let store = Store(URL(fileURLWithPath: CommandLine.arguments[3], isDirectory: true), mode)
report["mode"] = mode
var code: Int32 = 0
do {
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little_endian_required")
    let input = try Data(contentsOf: store.root.appendingPathComponent("cases.json"))
    try need(input.count < 131072, "manifest_size_bound")
    let manifest = try JSONDecoder().decode(Manifest.self, from: input)
    try validate(manifest)
    report["case_manifest_sha256"] = sha(input)
    let h = try Harness(URL(fileURLWithPath: CommandLine.arguments[2]), store)
    if mode == "blit-control" { try h.blitControl() }
    else { try h.observe(manifest, direct: readout == "direct") }
} catch {
    code = (error as? Failure)?.code ?? 3
    report["status"] = "FAILED_NOT_PASS"
    report["failure"] = errorInfo(error)
}
report["exit_code"] = code
do { try store.checkpoint() } catch { code = 4 }
print("native_mode=\(mode) native_exit=\(code)")
exit(code)
