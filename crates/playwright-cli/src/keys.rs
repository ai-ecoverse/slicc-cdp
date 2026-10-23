#[derive(Clone)]
pub struct Key {
    pub key: String,
    pub key_code: i64,
    pub code: String,
    pub text: String,
    pub location: i64,
    pub shifted: Option<Box<Key>>,
}

pub const MODIFIERS: &[&str] = &["Alt", "Control", "Meta", "Shift"];
pub const KEYPAD: i64 = 3;

type Row = (&'static str, i64, i64, &'static str, &'static str, i64, &'static str, i64);

const LAYOUT: &[Row] = &[
    ("Escape", 27, 0, "Escape", "", 0, "", 0),
    ("F1", 112, 0, "F1", "", 0, "", 0),
    ("F2", 113, 0, "F2", "", 0, "", 0),
    ("F3", 114, 0, "F3", "", 0, "", 0),
    ("F4", 115, 0, "F4", "", 0, "", 0),
    ("F5", 116, 0, "F5", "", 0, "", 0),
    ("F6", 117, 0, "F6", "", 0, "", 0),
    ("F7", 118, 0, "F7", "", 0, "", 0),
    ("F8", 119, 0, "F8", "", 0, "", 0),
    ("F9", 120, 0, "F9", "", 0, "", 0),
    ("F10", 121, 0, "F10", "", 0, "", 0),
    ("F11", 122, 0, "F11", "", 0, "", 0),
    ("F12", 123, 0, "F12", "", 0, "", 0),
    ("Backquote", 192, 0, "`", "~", 0, "", 0),
    ("Digit1", 49, 0, "1", "!", 0, "", 0),
    ("Digit2", 50, 0, "2", "@", 0, "", 0),
    ("Digit3", 51, 0, "3", "#", 0, "", 0),
    ("Digit4", 52, 0, "4", "$", 0, "", 0),
    ("Digit5", 53, 0, "5", "%", 0, "", 0),
    ("Digit6", 54, 0, "6", "^", 0, "", 0),
    ("Digit7", 55, 0, "7", "&", 0, "", 0),
    ("Digit8", 56, 0, "8", "*", 0, "", 0),
    ("Digit9", 57, 0, "9", "(", 0, "", 0),
    ("Digit0", 48, 0, "0", ")", 0, "", 0),
    ("Minus", 189, 0, "-", "_", 0, "", 0),
    ("Equal", 187, 0, "=", "+", 0, "", 0),
    ("Backslash", 220, 0, "\\", "|", 0, "", 0),
    ("Backspace", 8, 0, "Backspace", "", 0, "", 0),
    ("Tab", 9, 0, "Tab", "", 0, "", 0),
    ("KeyQ", 81, 0, "q", "Q", 0, "", 0),
    ("KeyW", 87, 0, "w", "W", 0, "", 0),
    ("KeyE", 69, 0, "e", "E", 0, "", 0),
    ("KeyR", 82, 0, "r", "R", 0, "", 0),
    ("KeyT", 84, 0, "t", "T", 0, "", 0),
    ("KeyY", 89, 0, "y", "Y", 0, "", 0),
    ("KeyU", 85, 0, "u", "U", 0, "", 0),
    ("KeyI", 73, 0, "i", "I", 0, "", 0),
    ("KeyO", 79, 0, "o", "O", 0, "", 0),
    ("KeyP", 80, 0, "p", "P", 0, "", 0),
    ("BracketLeft", 219, 0, "[", "{", 0, "", 0),
    ("BracketRight", 221, 0, "]", "}", 0, "", 0),
    ("CapsLock", 20, 0, "CapsLock", "", 0, "", 0),
    ("KeyA", 65, 0, "a", "A", 0, "", 0),
    ("KeyS", 83, 0, "s", "S", 0, "", 0),
    ("KeyD", 68, 0, "d", "D", 0, "", 0),
    ("KeyF", 70, 0, "f", "F", 0, "", 0),
    ("KeyG", 71, 0, "g", "G", 0, "", 0),
    ("KeyH", 72, 0, "h", "H", 0, "", 0),
    ("KeyJ", 74, 0, "j", "J", 0, "", 0),
    ("KeyK", 75, 0, "k", "K", 0, "", 0),
    ("KeyL", 76, 0, "l", "L", 0, "", 0),
    ("Semicolon", 186, 0, ";", ":", 0, "", 0),
    ("Quote", 222, 0, "'", "\"", 0, "", 0),
    ("Enter", 13, 0, "Enter", "", 0, "\r", 0),
    ("ShiftLeft", 160, 16, "Shift", "", 0, "", 1),
    ("KeyZ", 90, 0, "z", "Z", 0, "", 0),
    ("KeyX", 88, 0, "x", "X", 0, "", 0),
    ("KeyC", 67, 0, "c", "C", 0, "", 0),
    ("KeyV", 86, 0, "v", "V", 0, "", 0),
    ("KeyB", 66, 0, "b", "B", 0, "", 0),
    ("KeyN", 78, 0, "n", "N", 0, "", 0),
    ("KeyM", 77, 0, "m", "M", 0, "", 0),
    ("Comma", 188, 0, ",", "<", 0, "", 0),
    ("Period", 190, 0, ".", ">", 0, "", 0),
    ("Slash", 191, 0, "/", "?", 0, "", 0),
    ("ShiftRight", 161, 16, "Shift", "", 0, "", 2),
    ("ControlLeft", 162, 17, "Control", "", 0, "", 1),
    ("MetaLeft", 91, 0, "Meta", "", 0, "", 1),
    ("AltLeft", 164, 18, "Alt", "", 0, "", 1),
    ("Space", 32, 0, " ", "", 0, "", 0),
    ("AltRight", 165, 18, "Alt", "", 0, "", 2),
    ("AltGraph", 225, 0, "AltGraph", "", 0, "", 0),
    ("MetaRight", 92, 0, "Meta", "", 0, "", 2),
    ("ContextMenu", 93, 0, "ContextMenu", "", 0, "", 0),
    ("ControlRight", 163, 17, "Control", "", 0, "", 2),
    ("PrintScreen", 44, 0, "PrintScreen", "", 0, "", 0),
    ("ScrollLock", 145, 0, "ScrollLock", "", 0, "", 0),
    ("Pause", 19, 0, "Pause", "", 0, "", 0),
    ("PageUp", 33, 0, "PageUp", "", 0, "", 0),
    ("PageDown", 34, 0, "PageDown", "", 0, "", 0),
    ("Insert", 45, 0, "Insert", "", 0, "", 0),
    ("Delete", 46, 0, "Delete", "", 0, "", 0),
    ("Home", 36, 0, "Home", "", 0, "", 0),
    ("End", 35, 0, "End", "", 0, "", 0),
    ("ArrowLeft", 37, 0, "ArrowLeft", "", 0, "", 0),
    ("ArrowUp", 38, 0, "ArrowUp", "", 0, "", 0),
    ("ArrowRight", 39, 0, "ArrowRight", "", 0, "", 0),
    ("ArrowDown", 40, 0, "ArrowDown", "", 0, "", 0),
    ("AudioVolumeMute", 173, 0, "AudioVolumeMute", "", 0, "", 0),
    ("AudioVolumeDown", 174, 0, "AudioVolumeDown", "", 0, "", 0),
    ("AudioVolumeUp", 175, 0, "AudioVolumeUp", "", 0, "", 0),
    ("MediaTrackNext", 176, 0, "MediaTrackNext", "", 0, "", 0),
    ("MediaTrackPrevious", 177, 0, "MediaTrackPrevious", "", 0, "", 0),
    ("MediaPlayPause", 179, 0, "MediaPlayPause", "", 0, "", 0),
    ("NumLock", 144, 0, "NumLock", "", 0, "", 0),
    ("NumpadDivide", 111, 0, "/", "", 0, "", 3),
    ("NumpadMultiply", 106, 0, "*", "", 0, "", 3),
    ("NumpadSubtract", 109, 0, "-", "", 0, "", 3),
    ("Numpad7", 36, 0, "Home", "7", 103, "", 3),
    ("Numpad8", 38, 0, "ArrowUp", "8", 104, "", 3),
    ("Numpad9", 33, 0, "PageUp", "9", 105, "", 3),
    ("Numpad4", 37, 0, "ArrowLeft", "4", 100, "", 3),
    ("Numpad5", 12, 0, "Clear", "5", 101, "", 3),
    ("Numpad6", 39, 0, "ArrowRight", "6", 102, "", 3),
    ("NumpadAdd", 107, 0, "+", "", 0, "", 3),
    ("Numpad1", 35, 0, "End", "1", 97, "", 3),
    ("Numpad2", 40, 0, "ArrowDown", "2", 98, "", 3),
    ("Numpad3", 34, 0, "PageDown", "3", 99, "", 3),
    ("Numpad0", 45, 0, "Insert", "0", 96, "", 3),
    ("NumpadDecimal", 46, 0, "Delete", ".", 110, "", 3),
    ("NumpadEnter", 13, 0, "Enter", "", 0, "\r", 3),
];

const ALIASES: &[(&str, &[&str])] = &[
    ("ShiftLeft", &["Shift"]),
    ("ControlLeft", &["Control"]),
    ("AltLeft", &["Alt"]),
    ("MetaLeft", &["Meta"]),
    ("Enter", &["\n", "\r"]),
];

const MAC_COMMANDS: &[(&str, &[&str])] = &[
    ("Backspace", &["deleteBackward"]),
    ("Escape", &["cancelOperation"]),
    ("ArrowUp", &["moveUp"]),
    ("ArrowDown", &["moveDown"]),
    ("ArrowLeft", &["moveLeft"]),
    ("ArrowRight", &["moveRight"]),
    ("F5", &["complete"]),
    ("Delete", &["deleteForward"]),
    ("Home", &["scrollToBeginningOfDocument"]),
    ("End", &["scrollToEndOfDocument"]),
    ("PageUp", &["scrollPageUp"]),
    ("PageDown", &["scrollPageDown"]),
    ("Shift+Backspace", &["deleteBackward"]),
    ("Shift+Escape", &["cancelOperation"]),
    ("Shift+ArrowUp", &["moveUpAndModifySelection"]),
    ("Shift+ArrowDown", &["moveDownAndModifySelection"]),
    ("Shift+ArrowLeft", &["moveLeftAndModifySelection"]),
    ("Shift+ArrowRight", &["moveRightAndModifySelection"]),
    ("Shift+F5", &["complete"]),
    ("Shift+Delete", &["deleteForward"]),
    ("Shift+Home", &["moveToBeginningOfDocumentAndModifySelection"]),
    ("Shift+End", &["moveToEndOfDocumentAndModifySelection"]),
    ("Shift+PageUp", &["pageUpAndModifySelection"]),
    ("Shift+PageDown", &["pageDownAndModifySelection"]),
    ("Shift+Numpad5", &["delete"]),
    ("Control+Tab", &["selectNextKeyView"]),
    ("Control+KeyA", &["moveToBeginningOfParagraph"]),
    ("Control+KeyB", &["moveBackward"]),
    ("Control+KeyD", &["deleteForward"]),
    ("Control+KeyE", &["moveToEndOfParagraph"]),
    ("Control+KeyF", &["moveForward"]),
    ("Control+KeyH", &["deleteBackward"]),
    ("Control+KeyK", &["deleteToEndOfParagraph"]),
    ("Control+KeyL", &["centerSelectionInVisibleArea"]),
    ("Control+KeyN", &["moveDown"]),
    ("Control+KeyO", &["moveBackward"]),
    ("Control+KeyP", &["moveUp"]),
    ("Control+KeyT", &["transpose"]),
    ("Control+KeyV", &["pageDown"]),
    ("Control+KeyY", &["yank"]),
    ("Control+Backspace", &["deleteBackwardByDecomposingPreviousCharacter"]),
    ("Control+ArrowUp", &["scrollPageUp"]),
    ("Control+ArrowDown", &["scrollPageDown"]),
    ("Control+ArrowLeft", &["moveToLeftEndOfLine"]),
    ("Control+ArrowRight", &["moveToRightEndOfLine"]),
    ("Shift+Control+Tab", &["selectPreviousKeyView"]),
    ("Shift+Control+KeyA", &["moveToBeginningOfParagraphAndModifySelection"]),
    ("Shift+Control+KeyB", &["moveBackwardAndModifySelection"]),
    ("Shift+Control+KeyE", &["moveToEndOfParagraphAndModifySelection"]),
    ("Shift+Control+KeyF", &["moveForwardAndModifySelection"]),
    ("Shift+Control+KeyN", &["moveDownAndModifySelection"]),
    ("Shift+Control+KeyP", &["moveUpAndModifySelection"]),
    ("Shift+Control+KeyV", &["pageDownAndModifySelection"]),
    ("Shift+Control+Backspace", &["deleteBackwardByDecomposingPreviousCharacter"]),
    ("Shift+Control+ArrowUp", &["scrollPageUp"]),
    ("Shift+Control+ArrowDown", &["scrollPageDown"]),
    ("Shift+Control+ArrowLeft", &["moveToLeftEndOfLineAndModifySelection"]),
    ("Shift+Control+ArrowRight", &["moveToRightEndOfLineAndModifySelection"]),
    ("Alt+Backspace", &["deleteWordBackward"]),
    ("Alt+Escape", &["complete"]),
    ("Alt+ArrowUp", &["moveBackward", "moveToBeginningOfParagraph"]),
    ("Alt+ArrowDown", &["moveForward", "moveToEndOfParagraph"]),
    ("Alt+ArrowLeft", &["moveWordLeft"]),
    ("Alt+ArrowRight", &["moveWordRight"]),
    ("Alt+Delete", &["deleteWordForward"]),
    ("Alt+PageUp", &["pageUp"]),
    ("Alt+PageDown", &["pageDown"]),
    ("Shift+Alt+Backspace", &["deleteWordBackward"]),
    ("Shift+Alt+Escape", &["complete"]),
    ("Shift+Alt+ArrowUp", &["moveParagraphBackwardAndModifySelection"]),
    ("Shift+Alt+ArrowDown", &["moveParagraphForwardAndModifySelection"]),
    ("Shift+Alt+ArrowLeft", &["moveWordLeftAndModifySelection"]),
    ("Shift+Alt+ArrowRight", &["moveWordRightAndModifySelection"]),
    ("Shift+Alt+Delete", &["deleteWordForward"]),
    ("Shift+Alt+PageUp", &["pageUp"]),
    ("Shift+Alt+PageDown", &["pageDown"]),
    ("Control+Alt+KeyB", &["moveWordBackward"]),
    ("Control+Alt+KeyF", &["moveWordForward"]),
    ("Control+Alt+Backspace", &["deleteWordBackward"]),
    ("Shift+Control+Alt+KeyB", &["moveWordBackwardAndModifySelection"]),
    ("Shift+Control+Alt+KeyF", &["moveWordForwardAndModifySelection"]),
    ("Shift+Control+Alt+Backspace", &["deleteWordBackward"]),
    ("Meta+NumpadSubtract", &["cancel"]),
    ("Meta+Backspace", &["deleteToBeginningOfLine"]),
    ("Meta+ArrowUp", &["moveToBeginningOfDocument"]),
    ("Meta+ArrowDown", &["moveToEndOfDocument"]),
    ("Meta+ArrowLeft", &["moveToLeftEndOfLine"]),
    ("Meta+ArrowRight", &["moveToRightEndOfLine"]),
    ("Shift+Meta+NumpadSubtract", &["cancel"]),
    ("Shift+Meta+Backspace", &["deleteToBeginningOfLine"]),
    ("Shift+Meta+ArrowUp", &["moveToBeginningOfDocumentAndModifySelection"]),
    ("Shift+Meta+ArrowDown", &["moveToEndOfDocumentAndModifySelection"]),
    ("Shift+Meta+ArrowLeft", &["moveToLeftEndOfLineAndModifySelection"]),
    ("Shift+Meta+ArrowRight", &["moveToRightEndOfLineAndModifySelection"]),
    ("Meta+KeyA", &["selectAll"]),
    ("Meta+KeyC", &["copy"]),
    ("Meta+KeyX", &["cut"]),
    ("Meta+KeyV", &["paste"]),
    ("Meta+KeyZ", &["undo"]),
    ("Shift+Meta+KeyZ", &["redo"]),
];

fn entries() -> Vec<(String, Key)> {
    let mut out = Vec::new();
    for (code, key_code, without_location, key, shift_key, shift_key_code, text, location) in LAYOUT.iter().copied() {
        let single = key.chars().count() == 1;
        let base = Key {
            key: key.to_string(),
            key_code: if without_location != 0 { without_location } else { key_code },
            code: code.to_string(),
            text: if single { key.to_string() } else { text.to_string() },
            location,
            shifted: None,
        };
        let shifted = (!shift_key.is_empty()).then(|| Key {
            key: shift_key.to_string(),
            text: shift_key.to_string(),
            key_code: if shift_key_code != 0 { shift_key_code } else { base.key_code },
            ..base.clone()
        });
        out.push((code.to_string(), Key { shifted: shifted.clone().map(Box::new), ..base.clone() }));
        if let Some((_, names)) = ALIASES.iter().find(|(alias, _)| *alias == code) {
            for name in names.iter() {
                out.push((name.to_string(), base.clone()));
            }
        }
        if location != 0 {
            continue;
        }
        if single {
            out.push((key.to_string(), base.clone()));
        }
        if let Some(shifted) = shifted {
            out.push((shifted.key.clone(), shifted));
        }
    }
    out
}

pub fn lookup(name: &str) -> Option<Key> {
    entries().into_iter().rev().find(|(known, _)| known == name).map(|(_, key)| key)
}

pub fn smart(name: &str, mac: bool) -> &str {
    match name {
        "ControlOrMeta" if mac => "Meta",
        "ControlOrMeta" => "Control",
        other => other,
    }
}

pub fn mac_commands(code: &str, modifiers: &[String]) -> Vec<String> {
    let mut parts: Vec<&str> = ["Shift", "Control", "Alt", "Meta"].into_iter().filter(|name| modifiers.iter().any(|held| held == name)).collect();
    parts.push(code);
    let shortcut = parts.join("+");
    MAC_COMMANDS.iter().find(|(name, _)| *name == shortcut).map(|(_, commands)| commands.iter().map(|command| command.to_string()).collect()).unwrap_or_default()
}

pub fn mask(modifiers: &[String]) -> i64 {
    modifiers.iter().fold(0, |mask, name| mask | match name.as_str() {
        "Alt" => 1,
        "Control" => 2,
        "Meta" => 4,
        "Shift" => 8,
        _ => 0,
    })
}
