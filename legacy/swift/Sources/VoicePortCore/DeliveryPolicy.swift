import Foundation

public enum DeliveryDecision: Equatable, Sendable {
    case paste
    case retain(DeliveryReason)
}

public enum DeliveryReason: String, Equatable, Sendable {
    case automaticPasteOff, accessibilityMissing, targetChanged, focusChanged
    case unknownFocus, sensitiveInput, clipboardChanged, clipboardUnavailable

    public var message: String {
        switch self {
        case .automaticPasteOff: "自动粘贴已关闭，请在结果面板复制。"
        case .accessibilityMissing: "需要辅助功能权限才能自动粘贴；文字已保留。"
        case .targetChanged: "前台应用已改变，文字已保留，未投递。"
        case .focusChanged: "输入焦点已改变，文字已保留，未投递。"
        case .unknownFocus: "无法确认原输入框，文字已保留，请手动复制。"
        case .sensitiveInput: "当前为安全输入，文字已保留，未注入按键。"
        case .clipboardChanged: "剪贴板已被其他操作更新，文字已保留，未覆盖。"
        case .clipboardUnavailable: "无法完整保存剪贴板，文字已保留，请手动复制。"
        }
    }
}

public struct DeliveryContext: Sendable {
    public var automaticPaste: Bool
    public var accessibility: Bool
    public var sameApplication: Bool
    public var knownFocus: Bool
    public var sameFocus: Bool
    public var sensitiveInput: Bool
    public var sameClipboard: Bool
    public var restorableClipboard: Bool

    public init(automaticPaste: Bool, accessibility: Bool, sameApplication: Bool,
                knownFocus: Bool, sameFocus: Bool, sensitiveInput: Bool,
                sameClipboard: Bool, restorableClipboard: Bool) {
        self.automaticPaste = automaticPaste
        self.accessibility = accessibility
        self.sameApplication = sameApplication
        self.knownFocus = knownFocus
        self.sameFocus = sameFocus
        self.sensitiveInput = sensitiveInput
        self.sameClipboard = sameClipboard
        self.restorableClipboard = restorableClipboard
    }

    public var decision: DeliveryDecision {
        if !automaticPaste { return .retain(.automaticPasteOff) }
        if !accessibility { return .retain(.accessibilityMissing) }
        if sensitiveInput { return .retain(.sensitiveInput) }
        if !sameApplication { return .retain(.targetChanged) }
        if !knownFocus { return .retain(.unknownFocus) }
        if !sameFocus { return .retain(.focusChanged) }
        if !sameClipboard { return .retain(.clipboardChanged) }
        if !restorableClipboard { return .retain(.clipboardUnavailable) }
        return .paste
    }
}

/// Session identity prevents a cancelled request's late result from reaching another input session.
public struct SessionGate: Sendable {
    public private(set) var current: String?
    public init() {}
    public mutating func begin() -> String {
        let id = UUID().uuidString
        current = id
        return id
    }
    public mutating func cancel() { current = nil }
    public func accepts(_ id: String) -> Bool { current == id }
}
