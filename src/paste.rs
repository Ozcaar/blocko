use std::sync::Arc;

use anyhow::Result;
use bevy::{
    prelude::*,
    tasks::{IoTaskPool, Task, futures::check_ready},
};

use crate::{
    diagnostics::Diagnostic,
    edit::{EditRequest, Operation},
    editor::View,
    structure::Volume,
    world::{BlockState, BlockValue},
    world_write::{self, WorldSave},
};

#[derive(Clone)]
pub enum EditIntent {
    Paste {
        repeat: bool,
        confirm: bool,
    },
    Single,
    Region {
        volume: Volume,
        material: String,
        matching: Option<String>,
    },
}

impl EditIntent {
    fn needs_confirmation(&self) -> bool {
        matches!(
            self,
            Self::Paste { confirm: true, .. } | Self::Region { .. }
        )
    }
}

#[derive(Default)]
pub enum PasteState {
    #[default]
    Idle,
    Preparing(EditIntent, Task<Result<Operation>>),
    Ready(EditIntent, Operation),
    Saving(Task<Result<WorldSave>>),
    Saved(WorldSave),
    Edited(usize),
    Undone(usize),
    Redone(usize),
    Error(Diagnostic),
}

impl PasteState {
    pub fn locks_preview(&self) -> bool {
        matches!(
            self,
            Self::Preparing(..) | Self::Ready(..) | Self::Saving(_)
        )
    }

    pub fn writes(&self) -> bool {
        matches!(self, Self::Saving(_))
    }

    pub fn busy(&self) -> bool {
        matches!(self, Self::Preparing(..) | Self::Saving(_))
    }
}

#[derive(Clone, Copy)]
pub enum PasteAction {
    Prepare,
    Place { repeat: bool },
    Confirm,
    Discard,
}

pub fn apply_action(view: &mut View, action: PasteAction) {
    match action {
        PasteAction::Discard if !view.paste.writes() => view.paste = PasteState::Idle,
        PasteAction::Prepare | PasteAction::Place { .. } if !view.paste.locks_preview() => {
            let Some(preview) = view.preview.as_ref().filter(|preview| preview.target_valid) else {
                return;
            };
            let intent = match action {
                PasteAction::Place { repeat } => EditIntent::Paste {
                    repeat,
                    confirm: false,
                },
                _ => EditIntent::Paste {
                    repeat: false,
                    confirm: true,
                },
            };
            prepare(view, EditRequest::Paste(preview.clone()), intent);
        }
        PasteAction::Confirm if matches!(view.paste, PasteState::Ready(..)) => {
            if let PasteState::Ready(intent, operation) = std::mem::take(&mut view.paste) {
                stage(view, intent, operation);
            }
        }
        _ => {}
    }
}

pub fn set_blocks(
    view: &mut View,
    volume: Volume,
    state: BlockState,
    matching: Option<String>,
    region: bool,
) {
    if !view.paste.locks_preview() {
        let intent = if region {
            EditIntent::Region {
                volume,
                material: state.name.clone(),
                matching: matching.clone(),
            }
        } else {
            EditIntent::Single
        };
        let request = EditRequest::Set {
            volume,
            value: Arc::new(BlockValue {
                state,
                entity: None,
            }),
            matching,
        };
        prepare(view, request, intent);
    }
}

fn prepare(view: &mut View, request: EditRequest, intent: EditIntent) {
    let Some(stream) = &view.stream else { return };
    let world = stream.source();
    let snapshot = view.edits.patch_in(request.volume());
    view.paste = PasteState::Preparing(
        intent,
        IoTaskPool::get().spawn(async move { request.prepare(world, &snapshot) }),
    );
}

fn stage(view: &mut View, intent: EditIntent, operation: Operation) {
    view.paste = match view.edits.apply(operation) {
        Ok(count) => {
            match intent {
                EditIntent::Paste { repeat: true, .. } => {
                    if let Some(preview) = &mut view.preview {
                        preview.target_valid = !preview.moving;
                    }
                }
                EditIntent::Paste { .. } => view.preview = None,
                _ => {}
            }
            PasteState::Edited(count)
        }
        Err(error) => PasteState::Error(Diagnostic::from_error(error)),
    };
}

pub fn start_save(view: &mut View) -> bool {
    let Some(stream) = &view.stream else {
        return false;
    };
    if view.paste.busy() || view.copying || !view.edits.dirty() {
        return false;
    }
    let world = stream.source();
    let patch = view.edits.patch();
    view.paste = PasteState::Saving(
        IoTaskPool::get().spawn(async move { world_write::save_changes(world, &patch) }),
    );
    true
}

pub fn finish(mut commands: Commands, mut views: Query<&mut View>) {
    for mut view in &mut views {
        match &mut view.paste {
            PasteState::Preparing(intent, task) => {
                if let Some(result) = check_ready(task) {
                    let intent = intent.clone();
                    match result {
                        Ok(operation) if intent.needs_confirmation() => {
                            view.paste = PasteState::Ready(intent, operation);
                        }
                        Ok(operation) => stage(&mut view, intent, operation),
                        Err(error) => view.paste = PasteState::Error(Diagnostic::from_error(error)),
                    }
                }
            }
            PasteState::Saving(task) => {
                if let Some(result) = check_ready(task) {
                    view.paste = match result {
                        Ok(result) => {
                            view.edits.mark_saved();
                            if let Some(stream) = &mut view.stream {
                                stream.reload(&mut commands);
                            }
                            PasteState::Saved(result)
                        }
                        Err(error) => PasteState::Error(Diagnostic::from_error(error)),
                    };
                }
            }
            _ => {}
        }
    }
}
