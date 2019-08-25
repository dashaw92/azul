//! Text input (demonstrates two-way data binding)

use std::ops::Range;
use azul_core::{
    dom::{Dom, EventFilter, FocusEventFilter, TabIndex},
    window::{KeyboardState, VirtualKeyCode},
    callbacks::{Ref, Redraw, DefaultCallbackInfo, DefaultCallback, CallbackReturn},
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct TextInput<T> {
    pub on_text_input: DefaultCallback<T>,
    pub on_virtual_key_down: DefaultCallback<T>,
    pub state: Ref<TextInputState>,
}

impl<T> Default for TextInput<T> {
    fn default() -> Self {
        TextInput {
            on_text_input: DefaultCallback(Self::default_on_text_input),
            on_virtual_key_down: DefaultCallback(Self::default_on_virtual_key_down),
            state: Ref::default(),
        }
    }
}

impl<T> Into<Dom<T>> for TextInput<T> {
    fn into(self) -> Dom<T> {
        self.dom()
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct TextInputState {
    pub text: String,
    pub selection: Option<Selection>,
    pub cursor_pos: usize,
}

impl Default for TextInputState {
    fn default() -> Self {
        TextInputState {
            text: String::new(),
            selection: None,
            cursor_pos: 0,
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum Selection {
    All,
    FromTo(Range<usize>),
}

impl TextInputState {

    #[inline]
    pub fn new<S: Into<String>>(input: S) -> Self {
        Self {
            text: input.into(),
            selection: None,
            cursor_pos: 0,
        }
    }

    #[inline]
    pub fn with_cursor_pos(self, cursor_pos: usize) -> Self {
        Self { cursor_pos, .. self }
    }

    #[inline]
    pub fn with_selection(self, selection: Option<Selection>) -> Self {
        Self { selection, .. self }
    }

    pub fn handle_on_text_input(&mut self, keyboard_state: &KeyboardState) -> CallbackReturn {

        let c = keyboard_state.current_char?;

        match self.selection.clone() {
            None => {
                if self.cursor_pos == self.text.len() {
                    self.text.push(c);
                } else {
                    // TODO: insert character at the cursor location!
                    self.text.push(c);
                }
                self.cursor_pos = self.cursor_pos.saturating_add(1);
            },
            Some(Selection::All) => {
                self.text = format!("{}", c);
                self.cursor_pos = 1;
                self.selection = None;
            },
            Some(Selection::FromTo(range)) => {
                self.delete_selection(range, Some(c));
            },
        }

        Redraw
    }

    pub fn handle_on_virtual_key_down(&mut self, keyboard_state: &KeyboardState) -> CallbackReturn {

        let last_keycode = keyboard_state.current_virtual_keycode?;

        match last_keycode {
            VirtualKeyCode::Back => {
                // TODO: shift + back = delete last word
                let selection = self.selection.clone();
                match selection {
                    None => {
                        if self.cursor_pos == self.text.len() {
                            self.text.pop();
                        } else {
                            let mut a = self.text.chars().take(self.cursor_pos).collect::<String>();
                            let new = self.text.len().min(self.cursor_pos.saturating_add(1));
                            a.extend(self.text.chars().skip(new));
                            self.text = a;
                        }
                        self.cursor_pos = self.cursor_pos.saturating_sub(1);
                    },
                    Some(Selection::All) => {
                        self.text.clear();
                        self.cursor_pos = 0;
                        self.selection = None;
                    },
                    Some(Selection::FromTo(range)) => {
                        self.delete_selection(range, None);
                    },
                }
            },
            VirtualKeyCode::Return => {
                // TODO: selection!
                self.text.push('\n');
                self.cursor_pos = self.cursor_pos.saturating_add(1);
            },
            VirtualKeyCode::Home => {
                self.cursor_pos = 0;
                self.selection = None;
            },
            VirtualKeyCode::End => {
                self.cursor_pos = self.text.len();
                self.selection = None;
            },
            VirtualKeyCode::Escape => {
                self.selection = None;
            },
            VirtualKeyCode::Right => {
                self.cursor_pos = self.text.len().min(self.cursor_pos.saturating_add(1));
            },
            VirtualKeyCode::Left => {
                self.cursor_pos = (0.max(self.cursor_pos.saturating_sub(1))).min(self.cursor_pos.saturating_add(1));
            },
            VirtualKeyCode::A if keyboard_state.ctrl_down => {
                self.selection = Some(Selection::All);
            },
            VirtualKeyCode::C if keyboard_state.ctrl_down => {},
            VirtualKeyCode::V if keyboard_state.ctrl_down => {},
            _ => { },
        }

        Redraw
    }

    pub fn delete_selection(&mut self, selection: Range<usize>, new_text: Option<char>) {
        let Range { start, end } = selection;
        let max = if end > self.text.len() { self.text.len() } else { end };

        let mut cur = start;
        if max == self.text.len() {
            self.text.truncate(start);
        } else {
            let mut a = self.text.chars().take(start).collect::<String>();

            if let Some(new) = new_text {
                a.push(new);
                cur += 1;
            }

            a.extend(self.text.chars().skip(end));
            self.text = a;
        }

        self.cursor_pos = cur;
    }
}

impl<T> TextInput<T> {

    pub fn new(state: Ref<TextInputState>) -> Self {
        Self { state, .. Default::default() }
    }

    pub fn with_state(self, state: Ref<TextInputState>) -> Self {
        Self { state, .. self }
    }

    pub fn on_text_input(self, callback: DefaultCallback<T>) -> Self {
        Self { on_text_input: callback, .. self }
    }

    pub fn on_virtual_key_down(self, callback: DefaultCallback<T>) -> Self {
        Self { on_text_input: callback, .. self }
    }

    pub fn dom(self) -> Dom<T> {

        let label = Dom::label(self.state.borrow().text.clone())
            .with_class("__azul-native-input-text-label");

        let upcasted_state = self.state.upcast();

        Dom::div()
            .with_class("__azul-native-input-text")
            .with_tab_index(TabIndex::Auto)
            .with_default_callback(EventFilter::Focus(FocusEventFilter::TextInput), self.on_text_input, upcasted_state.clone())
            .with_default_callback(EventFilter::Focus(FocusEventFilter::VirtualKeyDown), self.on_virtual_key_down, upcasted_state)
            .with_child(label)
    }

    pub fn default_on_text_input(info: DefaultCallbackInfo<T>) -> CallbackReturn {
        let text_input_state = info.state.downcast::<TextInputState>()?;
        let keyboard_state = info.current_window_state.get_keyboard_state();
        text_input_state.borrow_mut().handle_on_text_input(keyboard_state)
    }

    pub fn default_on_virtual_key_down(info: DefaultCallbackInfo<T>) -> CallbackReturn {
        let text_input_state = info.state.downcast::<TextInputState>()?;
        let keyboard_state = info.current_window_state.get_keyboard_state();
        text_input_state.borrow_mut().handle_on_virtual_key_down(keyboard_state)
    }
}
