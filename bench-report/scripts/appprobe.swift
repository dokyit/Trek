// appprobe: find an app's processes (by bundle path or responsible pid) and time its first window.
// usage: appprobe pids <bundle.app>
//        appprobe launch <bundle.app> [timeout_s]   -> ms from `open -g` to first on-screen window >= 200x150
//        appprobe windows <bundle.app>              -> count of on-screen windows
import Foundation
import CoreGraphics
import Darwin

typealias RespFn = @convention(c) (pid_t) -> pid_t
let resp: RespFn? = {
    guard let h = dlopen(nil, RTLD_NOW), let s = dlsym(h, "responsibility_get_pid_responsible_for_pid") else { return nil }
    return unsafeBitCast(s, to: RespFn.self)
}()

func allPids() -> [pid_t] {
    let n = proc_listallpids(nil, 0)
    var buf = [pid_t](repeating: 0, count: Int(n) * 2)
    let m = proc_listallpids(&buf, Int32(buf.count * MemoryLayout<pid_t>.size))
    return Array(buf.prefix(Int(m))).filter { $0 > 0 }
}
func path(_ p: pid_t) -> String {
    var b = [CChar](repeating: 0, count: 4096)
    return proc_pidpath(p, &b, UInt32(b.count)) > 0 ? String(cString: b) : ""
}
func appPids(_ bundle: String) -> [pid_t] {
    let pref = bundle.hasSuffix("/") ? bundle : bundle + "/"
    let pids = allPids()
    let own = Set(pids.filter { path($0).hasPrefix(pref) })
    var out = own
    if let r = resp { for p in pids where own.contains(r(p)) { out.insert(p) } }
    return out.sorted()
}
func windows(_ pids: Set<pid_t>) -> Int {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else { return 0 }
    return list.filter { w in
        guard let pid = w[kCGWindowOwnerPID as String] as? pid_t, pids.contains(pid),
              let layer = w[kCGWindowLayer as String] as? Int, layer == 0,
              let b = w[kCGWindowBounds as String] as? [String: Double] else { return false }
        return (b["Width"] ?? 0) >= 200 && (b["Height"] ?? 0) >= 150
    }.count
}
let a = CommandLine.arguments
guard a.count >= 3 else { print("usage"); exit(2) }
let bundle = a[2]
switch a[1] {
case "pids": print(appPids(bundle).map(String.init).joined(separator: " "))
case "windows": print(windows(Set(appPids(bundle))))
case "winid", "winid-pid":
    let pids: Set<pid_t> = a[1] == "winid-pid" ? [pid_t(bundle)!] : Set(appPids(bundle))
    let list = (CGWindowListCopyWindowInfo(a[1] == "winid-pid" ? [.optionAll] : [.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]]) ?? []
    var best: (Int, Double) = (0, 0)
    for w in list {
        guard let pid = w[kCGWindowOwnerPID as String] as? pid_t, pids.contains(pid),
              let layer = w[kCGWindowLayer as String] as? Int, layer == 0,
              let b = w[kCGWindowBounds as String] as? [String: Double], let id = w[kCGWindowNumber as String] as? Int else { continue }
        let area = (b["Width"] ?? 0) * (b["Height"] ?? 0)
        if area > best.1 { best = (id, area) }
    }
    print(best.0)
case "launch":
    let timeout = a.count > 3 ? Double(a[3])! : 60
    let t0 = Date()
    let p = Process(); p.executableURL = URL(fileURLWithPath: "/usr/bin/open"); p.arguments = ["-g", bundle]
    try! p.run()
    while Date().timeIntervalSince(t0) < timeout {
        if windows(Set(appPids(bundle))) > 0 { print(Int(Date().timeIntervalSince(t0) * 1000)); exit(0) }
        usleep(10_000)
    }
    print("timeout"); exit(1)
default: print("usage"); exit(2)
}
