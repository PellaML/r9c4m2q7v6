import Foundation
import Metal

guard CommandLine.arguments.count == 4 else { exit(2) }
let label = CommandLine.arguments[1]
guard ["q1-shared", "q1-managed", "q1-private", "q2"].contains(label) else { exit(2) }
let source = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
let out = URL(fileURLWithPath: CommandLine.arguments[3], isDirectory: true)
for suffix in [".json", ".gpu.bin"] {
    guard !FileManager.default.fileExists(atPath: out.appendingPathComponent(label + suffix).path) else { exit(2) }
}
let s = Session(source, out, label)
var code: Int32 = 0
do {
    try s.prepare()
    if label == "q2" { try question2(s) }
    else {
        let mode: MTLStorageMode = label == "q1-shared" ? .shared : (label == "q1-managed" ? .managed : .private)
        try question1(s, mode)
    }
    s.report["status"] = "CAPTURED_PENDING_INDEPENDENT_VALIDATION"
} catch { s.report["failure"] = errorInfo(error); code = 3 }
s.report["exit_code"] = code
do { try s.checkpoint("finished") } catch { code = 4 }
print("native_exit=\(code)")
exit(code)
