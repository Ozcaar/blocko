use std::{ptr, slice};

use bevy_egui::egui::{Event, Key, OutputCommand, RawInput};
use windows_sys::Win32::{
    Foundation::{GlobalFree, HWND},
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
        },
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
    },
};

const UNICODE_TEXT: u32 = 13; // CF_UNICODETEXT

struct ClipboardAccess;

impl ClipboardAccess {
    fn open(window: HWND) -> Option<Self> {
        // The caller passes the HWND of the live editor window.
        (unsafe { OpenClipboard(window) } != 0).then_some(Self)
    }
}

impl Drop for ClipboardAccess {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

pub fn add_text_events(input: &mut RawInput, window: HWND) {
    let mut events = Vec::new();
    for event in &input.events {
        if let Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = event
            && modifiers.command
            && !modifiers.alt
        {
            match key {
                Key::C => events.push(Event::Copy),
                Key::X => events.push(Event::Cut),
                Key::V => {
                    if let Some(text) = read(window) {
                        events.push(Event::Paste(text));
                    }
                }
                _ => {}
            }
        }
    }
    input.events.extend(events);
}

pub fn copy_text(commands: &[OutputCommand], window: HWND) {
    if let Some(OutputCommand::CopyText(text)) = commands
        .iter()
        .rev()
        .find(|command| matches!(command, OutputCommand::CopyText(text) if !text.is_empty()))
    {
        write(window, text);
    }
}

fn read(window: HWND) -> Option<String> {
    let _clipboard = ClipboardAccess::open(window)?;
    // Clipboard data is valid while access is held. Bound the read by its allocation.
    unsafe {
        let memory = GetClipboardData(UNICODE_TEXT);
        if memory.is_null() {
            return None;
        }
        let length = (GlobalSize(memory) / size_of::<u16>()).min(1_048_576);
        let data = GlobalLock(memory).cast::<u16>();
        if data.is_null() {
            return None;
        }
        let data = slice::from_raw_parts(data, length);
        let text = data
            .iter()
            .position(|&value| value == 0)
            .map(|end| String::from_utf16_lossy(&data[..end]));
        GlobalUnlock(memory);
        text
    }
}

fn write(window: HWND, value: &str) {
    let Some(_clipboard) = ClipboardAccess::open(window) else {
        return;
    };
    let text = value.encode_utf16().chain([0]).collect::<Vec<_>>();
    // The OS owns this allocation only after SetClipboardData succeeds.
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, text.len() * size_of::<u16>());
        if memory.is_null() {
            return;
        }
        let data = GlobalLock(memory).cast::<u16>();
        if !data.is_null() {
            ptr::copy_nonoverlapping(text.as_ptr(), data, text.len());
            GlobalUnlock(memory);
            if EmptyClipboard() != 0 && !SetClipboardData(UNICODE_TEXT, memory).is_null() {
                return;
            }
        }
        GlobalFree(memory);
    }
}
