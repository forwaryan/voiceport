// Build with xcrun swiftc; launch only as a dedicated test App bundle.
// Usage: fixture standard|web-textarea|web-contenteditable ABSOLUTE_NEW_RECEIPT
// A dedicated synthetic receiver. Local HTML only; never records or sends data.
import Cocoa
import WebKit

let arguments = Array(CommandLine.arguments.dropFirst())
precondition(arguments.count == 2, "MODE ABSOLUTE_NEW_RECEIPT")
let mode = arguments[0]
precondition(["standard", "web-textarea", "web-contenteditable"].contains(mode))
let receipt = URL(fileURLWithPath: arguments[1])
precondition(arguments[1].hasPrefix("/") && !FileManager.default.fileExists(atPath: receipt.path))

func save(_ text: String, ready: Bool, selection: [Int] = []) {
    let data = try! JSONSerialization.data(withJSONObject: [
        "pid": ProcessInfo.processInfo.processIdentifier,
        "mode": mode, "text": text, "ready": ready, "selection": selection
    ], options: [.sortedKeys])
    try! data.write(to: receipt, options: [.atomic])
}

final class Navigation: NSObject, WKNavigationDelegate {
    var prepared = false
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        webView.evaluateJavaScript("""
        const e = document.getElementById('target'); e.focus();
        if (e.tagName === 'TEXTAREA') { e.setSelectionRange(1, 2); }
        else {
          const r = document.createRange(); r.setStart(e.firstChild, 1); r.setEnd(e.firstChild, 2);
          const s = window.getSelection(); s.removeAllRanges(); s.addRange(r);
        }
        true;
        """) { _, error in
            precondition(error == nil)
            self.prepared = true
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 640, height: 300),
    styleMask: [.titled, .closable], backing: .buffered, defer: false)
window.isReleasedWhenClosed = false
window.title = "VoicePort · 专用合成输入目标（无录音）"
let edit = NSMenu(title: "Edit")
edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
let item = NSMenuItem(); item.submenu = edit
let menu = NSMenu(); menu.addItem(item); app.mainMenu = menu
let text = NSTextView(frame: NSRect(x: 0, y: 0, width: 640, height: 300))
let navigation = Navigation()
let web = WKWebView(frame: NSRect(x: 0, y: 0, width: 640, height: 300))
web.navigationDelegate = navigation
if mode == "standard" {
    text.string = "左|右"
    window.contentView = text
} else {
    window.contentView = web
    let field = mode == "web-textarea" ? "<textarea id='target'>左|右</textarea>" : "<div id='target' contenteditable='true'>左|右</div>"
    web.loadHTMLString("""
    <!doctype html><html><head><meta charset="utf-8">
    <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">
    <style>body { margin:24px; font:20px sans-serif } #target { width:560px; min-height:180px; border:1px solid #777; padding:8px }</style>
    </head><body>\(field)</body></html>
    """, baseURL: nil)
}
window.center(); window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
if mode == "standard" {
    window.makeFirstResponder(text)
    text.setSelectedRange(NSRange(location: 1, length: 1))
    save(text.string, ready: true, selection: [1, 1])
} else {
    precondition(window.makeFirstResponder(web), "WebKit must own native keyboard focus")
}
let started = Date()
let timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { _ in
    if Date().timeIntervalSince(started) > 45 { exit(0) }
    if mode == "standard" {
        let selected = text.selectedRange()
        save(text.string, ready: true, selection: [selected.location, selected.length])
    } else if navigation.prepared {
        web.evaluateJavaScript("""
        (() => { const e=document.getElementById('target'); return {text:e.tagName==='TEXTAREA'?e.value:e.textContent,focused:document.activeElement===e && document.hasFocus()}; })()
        """) { value, error in
            guard error == nil, let object = value as? [String: Any], let value = object["text"] as? String else { return }
            save(value, ready: object["focused"] as? Bool == true)
        }
    }
}
app.run()
