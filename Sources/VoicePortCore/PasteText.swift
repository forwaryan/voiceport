import Foundation

public enum PasteText {
    /// Preserve the original result separately. Automatic paste must not contain terminal control characters or line breaks.
    public static func automatic(_ original: String) -> String {
        original.map { character in
            if character.isNewline || character.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) {
                return " "
            }
            return String(character)
        }.joined()
    }
}
