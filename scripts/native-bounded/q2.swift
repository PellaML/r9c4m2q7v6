import Foundation
import Metal

func question2(_ s: Session) throws {
    let source = try s.input("q2-source.bin", 4096), seed = try s.input("q2-seed.bin", 8192)
    let m = try JSONDecoder().decode(Manifest.self, from: s.input("cases.json", 131072))
    try need(source.count == rowCount*8 && seed.count == targetBytes && m.q2.count == 12, "q2_input_bounds")
    for (i, c) in m.q2.enumerated() {
        try need(c.id == i+1 && [16, 32].contains(c.precision) && ["mul", "replace"].contains(c.mode) &&
            [0, 8, 12].contains(c.mask) && c.name.range(of: "^[a-z0-9-]{1,48}$", options: .regularExpression) != nil &&
            ["output_float", "output_half", "wrong_kernel", "wrong_output"].contains(c.fragment) &&
            ["", "wrong-kernel", "wrong-output", "wrong-state", "wrong-mask"].contains(c.negative), "q2_spec_bounds")
    }
    try need(Set(m.q2.map { $0.name }).count == 12, "q2_duplicate_names")
    s.report["input_sha256"] = ["source": sha(source), "seed": sha(seed)]
    s.report["layout"] = ["width": width, "height": height, "active_x": 1, "active_y": 1,
        "active_width": innerWidth, "active_height": 9, "pitch": pitch, "offset": guardBytes,
        "trace_prefix_suffix": traceGuard, "trace_stride": 16, "vectors": rowCount,
        "padding_scope": "host-visible blit-buffer row/prefix/suffix only, not opaque texture allocation padding"]
    s.report["channel_scope"] = ["declared_fragment_returns": ["float2", "half2"], "target_channels": "RG only",
        "explicit_BA_shader_access": false, "destination_texture_bound_to_fragment": false,
        "alpha_factor_configured_but_no_stored_alpha_to_observe": true,
        "physical_blend_precision_or_hidden_BA_access_proven": false]
    let upload = try s.uploaded(seed), inputs = try s.buffer(source.count + traceGuard*2)
    source.withUnsafeBytes { inputs.contents().advanced(by: traceGuard).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let inputBefore = snapshot(inputs)
    let size = MTLSize(width: width, height: height, depth: 1), origin = MTLOrigin(x: 0, y: 0, z: 0)
    func texture() throws -> MTLTexture {
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rg16Float, width: width, height: height, mipmapped: false)
        d.storageMode = .private; d.usage = .renderTarget; d.hazardTrackingMode = .tracked
        try s.checkpoint("before_rg16_texture")
        guard let t = s.dev.makeTexture(descriptor: d) else { throw Failure(reason: "rg16_target_nil_unsupported_no_substitute") }
        s.report["last_target"] = textureInfo(t); try s.checkpoint("rg16_texture_created")
        try need(t.pixelFormat == .rg16Float && t.storageMode == .private && t.usage == .renderTarget &&
            t.width == width && t.height == height && t.depth == 1 && t.mipmapLevelCount == 1 &&
            t.arrayLength == 1 && t.sampleCount == 1 && t.textureType == .type2D && t.buffer == nil &&
            !t.isFramebufferOnly && t.hazardTrackingMode == .tracked, "rg16_actual_target_properties")
        return t
    }
    func copyIn(_ cb: MTLCommandBuffer, _ t: MTLTexture) throws {
        guard let e = cb.makeBlitCommandEncoder() else { throw Failure(reason: "q2_upload_encoder_nil") }
        e.copy(from: upload, sourceOffset: guardBytes, sourceBytesPerRow: pitch, sourceBytesPerImage: pitch*height,
            sourceSize: size, to: t, destinationSlice: 0, destinationLevel: 0, destinationOrigin: origin)
        e.endEncoding()
    }
    func copyOut(_ cb: MTLCommandBuffer, _ t: MTLTexture, _ b: MTLBuffer) throws {
        guard let e = cb.makeBlitCommandEncoder() else { throw Failure(reason: "q2_readback_encoder_nil") }
        e.copy(from: t, sourceSlice: 0, sourceLevel: 0, sourceOrigin: origin, sourceSize: size,
            to: b, destinationOffset: guardBytes, destinationBytesPerRow: pitch, destinationBytesPerImage: pitch*height)
        e.endEncoding()
    }
    do {
        let t = try texture(), b = try s.buffer(targetBytes), cb = try s.command()
        try copyIn(cb, t); try copyOut(cb, t, b); try s.finish(cb, "baseline")
        let d = snapshot(b)
        try s.save(d, ["kind": "target", "name": "baseline", "tag": 0, "actual_texture": textureInfo(t)])
        try need(d == seed, "q2_raw_seed_roundtrip_not_exact")
    }
    for c in m.q2 {
        let d = MTLRenderPipelineDescriptor()
        d.rasterSampleCount = 1; d.isAlphaToCoverageEnabled = false; d.isAlphaToOneEnabled = false
        d.isRasterizationEnabled = true
        guard let v = s.library.makeFunction(name: "full_triangle"), let f = s.library.makeFunction(name: c.fragment) else {
            throw Failure(reason: "q2_function_missing")
        }
        d.vertexFunction = v; d.fragmentFunction = f
        let a = d.colorAttachments[0]!
        a.pixelFormat = .rg16Float; a.isBlendingEnabled = true
        a.sourceRGBBlendFactor = c.mode == "mul" ? .destinationColor : .one
        a.destinationRGBBlendFactor = .zero; a.rgbBlendOperation = .add
        a.sourceAlphaBlendFactor = .destinationAlpha; a.destinationAlphaBlendFactor = .zero; a.alphaBlendOperation = .add
        a.writeMask = MTLColorWriteMask(rawValue: c.mask)
        let state: [String: Any] = ["format": a.pixelFormat.rawValue, "mask": a.writeMask.rawValue,
            "blending": a.isBlendingEnabled, "rgb_op": a.rgbBlendOperation.rawValue, "alpha_op": a.alphaBlendOperation.rawValue,
            "src_rgb": a.sourceRGBBlendFactor.rawValue, "dst_rgb": a.destinationRGBBlendFactor.rawValue,
            "src_alpha": a.sourceAlphaBlendFactor.rawValue, "dst_alpha": a.destinationAlphaBlendFactor.rawValue,
            "samples": d.rasterSampleCount, "fragment": c.fragment, "vertex": "full_triangle",
            "viewport": [0, 0, width, height, 0, 1], "scissor": [1, 1, innerWidth, 9],
            "load": "load", "store": "store", "depth_stencil": "none", "cull": "none",
            "descriptor_receipt_not_opaque_pipeline_introspection": true]
        s.report["pending_pipeline"] = state; try s.checkpoint("before_pipeline_" + c.name)
        let pipeline = try s.dev.makeRenderPipelineState(descriptor: d)
        let t = try texture(), b = try s.buffer(targetBytes), trace = try s.buffer(traceGuard*2 + rowCount*16)
        let cb = try s.command(); try copyIn(cb, t)
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = t; pass.colorAttachments[0].loadAction = .load; pass.colorAttachments[0].storeAction = .store
        guard let e = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(reason: "q2_render_encoder_nil") }
        e.setRenderPipelineState(pipeline); e.setCullMode(.none); e.setTriangleFillMode(.fill)
        e.setFrontFacing(.clockwise)
        e.setViewport(MTLViewport(originX: 0, originY: 0, width: Double(width), height: Double(height), znear: 0, zfar: 1))
        e.setScissorRect(MTLScissorRect(x: 1, y: 1, width: innerWidth, height: 9))
        e.setFragmentBuffer(inputs, offset: traceGuard, index: 0); e.setFragmentBuffer(trace, offset: traceGuard, index: 1)
        var p = SIMD4<UInt32>(UInt32(rowCount), UInt32(c.id), 0, 0)
        e.setFragmentBytes(&p, length: 16, index: 2)
        e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3); e.endEncoding()
        try copyOut(cb, t, b); try s.finish(cb, c.name)
        let fields: [String: Any] = ["name": c.name, "tag": c.id, "precision": c.precision,
            "mode": c.mode, "mask": c.mask, "negative": c.negative, "state": state, "actual_texture": textureInfo(t)]
        var tr = fields; tr["kind"] = "target"; try s.save(snapshot(b), tr)
        tr["kind"] = "trace"; try s.save(snapshot(trace), tr)
        try need(snapshot(upload) == seed && snapshot(inputs) == inputBefore, "q2_input_buffer_or_guard_changed")
    }
    try need(s.commands.count == 13 && s.records.count == 25, "q2_coverage_incomplete")
    s.report["input_buffers_unchanged"] = true
}
