import Cocoa
import ApplicationServices

// Small accessibility helper for deterministic UI checks.
//
//   swift scripts/dev/ax.swift <pid> list
//   swift scripts/dev/ax.swift <pid> windows
//   swift scripts/dev/ax.swift <pid> press "<button title>"
//   swift scripts/dev/ax.swift <pid> set "<field description or value>" "<value>"
//   swift scripts/dev/ax.swift <pid> read "<field description or value>"

let pid = pid_t(CommandLine.arguments[1])!
let action = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "list"
let key = CommandLine.arguments.count > 3 ? CommandLine.arguments[3] : ""
let value = CommandLine.arguments.count > 4 ? CommandLine.arguments[4] : ""
let app = AXUIElementCreateApplication(pid)

func attr(_ element: AXUIElement, _ name: String) -> AnyObject? {
    var result: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &result) == .success ? result : nil
}

func text(_ element: AXUIElement, _ name: String) -> String {
    if let value = attr(element, name) as? String { return value }
    if let value = attr(element, name) as? NSNumber { return value.stringValue }
    return ""
}

func windows() -> [AXUIElement] {
    attr(app, kAXWindowsAttribute) as? [AXUIElement] ?? []
}

func matches(_ element: AXUIElement, _ key: String) -> Bool {
    text(element, kAXTitleAttribute) == key
        || text(element, kAXDescriptionAttribute) == key
        || text(element, kAXValueAttribute) == key
}

func find(_ element: AXUIElement, _ key: String, role: String? = nil) -> AXUIElement? {
    if matches(element, key) {
        if role == nil || text(element, kAXRoleAttribute) == role! {
            return element
        }
    }
    if let children = attr(element, kAXChildrenAttribute) as? [AXUIElement] {
        for child in children {
            if let found = find(child, key, role: role) { return found }
        }
    }
    return nil
}

func find_in_windows(_ key: String, role: String? = nil) -> AXUIElement? {
    for window in windows() {
        if let found = find(window, key, role: role) { return found }
    }
    return nil
}

func dump(_ element: AXUIElement, _ depth: Int) {
    if depth > 14 { return }
    let role = text(element, kAXRoleAttribute)
    let title = text(element, kAXTitleAttribute)
    let description = text(element, kAXDescriptionAttribute)
    let current = text(element, kAXValueAttribute)
    print("\(String(repeating: "  ", count: depth))\(role) title=[\(title)] value=[\(current)] desc=[\(description)]")
    if let children = attr(element, kAXChildrenAttribute) as? [AXUIElement] {
        for child in children { dump(child, depth + 1) }
    }
}

switch action {
case "list":
    for window in windows() { dump(window, 0) }
case "windows":
    for window in windows() {
        print(text(window, kAXTitleAttribute), text(window, kAXRoleAttribute))
    }
case "press":
    guard let element = find_in_windows(key, role: "AXButton") else {
        print("not found")
        exit(1)
    }
    print(AXUIElementPerformAction(element, kAXPressAction as CFString).rawValue)
case "select":
    guard let element = find_in_windows(key) else {
        print("not found")
        exit(1)
    }
    var row: AXUIElement? = element
    while let current = row, text(current, kAXRoleAttribute) != "AXRow" {
        row = (attr(current, kAXParentAttribute) as! AXUIElement?)
    }
    guard let row = row else {
        print("no row")
        exit(1)
    }
    print(AXUIElementSetAttributeValue(row, kAXSelectedAttribute as CFString, kCFBooleanTrue).rawValue)
case "set", "read":
    guard let element = find_in_windows(key) else {
        print("not found")
        exit(1)
    }
    if action == "set" {
        print(AXUIElementSetAttributeValue(element, kAXValueAttribute as CFString, value as CFString).rawValue)
    } else {
        print(text(element, kAXValueAttribute))
    }
default:
    print("unknown action")
    exit(2)
}
