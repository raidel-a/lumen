import CoreGraphics

// Prints the window number for an owner name and optional title.
let owner = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ""
let title = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : ""
let ownerPid = Int(owner)
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
for window in list {
    let name = window[kCGWindowOwnerName as String] as? String ?? ""
    let caption = window[kCGWindowName as String] as? String ?? ""
    if let ownerPid = ownerPid {
        guard (window[kCGWindowOwnerPID as String] as? Int) == ownerPid else { continue }
    } else {
        guard name.lowercased() == owner.lowercased() else { continue }
    }
    if !title.isEmpty && caption != title { continue }
    if let number = window[kCGWindowNumber as String] as? Int {
        print(number)
        exit(0)
    }
}
exit(1)
