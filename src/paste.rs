use std::sync::Arc;

use anyhow::Result;
use bevy::{
    prelude::*,
    tasks::{IoTaskPool, Task, futures::check_ready},
};

pub use crate::world_write::{PastePlan, PasteResult, PasteSummary};
use crate::{diagnostics::Diagnostic, editor::View, world_write};

#[derive(Default)]
pub enum PasteState {
    #[default]
    Idle,
    Preparing(Option<bool>, Task<Result<PastePlan>>),
    Ready(PastePlan),
    Saving(bool, Task<Result<PasteResult>>),
    Saved(PasteResult),
    Error(Diagnostic),
    TargetChanged,
}

impl PasteState {
    pub fn locks_preview(&self) -> bool {
        matches!(
            self,
            Self::Preparing(..) | Self::Ready(_) | Self::Saving(..)
        )
    }

    pub fn writes(&self) -> bool {
        matches!(self, Self::Preparing(Some(_), _) | Self::Saving(..))
    }

    pub fn is_saving(&self) -> bool {
        matches!(self, Self::Saving(..))
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
        PasteAction::Discard if !view.paste.writes() => {
            view.paste = PasteState::Idle;
        }
        PasteAction::Prepare | PasteAction::Place { .. } if !view.paste.locks_preview() => {
            let (Some(preview), Some(stream)) = (view.preview.as_mut(), view.stream.as_ref())
            else {
                return;
            };
            let repeat = match action {
                PasteAction::Place { repeat } => Some(repeat),
                _ => None,
            };
            if !preview.target_valid {
                return;
            }
            view.selection.picking = None;
            view.selection.pick_b_after_a = false;
            let preview = preview.clone();
            let world = stream.source();
            view.paste = PasteState::Preparing(
                repeat,
                IoTaskPool::get().spawn(async move { world_write::prepare(world, preview) }),
            );
        }
        PasteAction::Confirm if matches!(view.paste, PasteState::Ready(_)) => {
            let PasteState::Ready(plan) = std::mem::take(&mut view.paste) else {
                return;
            };
            save(view, plan, false);
        }
        _ => {}
    }
}

fn save(view: &mut View, plan: PastePlan, repeat: bool) {
    if !view
        .stream
        .as_ref()
        .is_some_and(|stream| Arc::ptr_eq(&stream.source(), &plan.world))
        || !view
            .preview
            .as_ref()
            .is_some_and(|preview| preview.same_mask(&plan.preview))
    {
        view.paste = PasteState::TargetChanged;
        return;
    }
    view.paste = PasteState::Saving(
        repeat,
        IoTaskPool::get().spawn(async move { world_write::save(plan) }),
    );
}

pub fn finish(mut commands: Commands, mut views: Query<&mut View>) {
    for mut view in &mut views {
        match &mut view.paste {
            PasteState::Preparing(repeat, task) => {
                let repeat = *repeat;
                if let Some(result) = check_ready(task) {
                    match (result, repeat) {
                        (Ok(plan), Some(repeat)) => save(&mut view, plan, repeat),
                        (Ok(plan), None) => view.paste = PasteState::Ready(plan),
                        (Err(error), _) => {
                            view.paste = PasteState::Error(Diagnostic::from_error(error))
                        }
                    }
                }
            }
            PasteState::Saving(repeat, task) => {
                let repeat = *repeat;
                if let Some(result) = check_ready(task) {
                    view.paste = match result {
                        Ok(result) => {
                            if repeat {
                                if let Some(preview) = &mut view.preview {
                                    preview.target_valid = !preview.moving;
                                }
                            } else {
                                view.preview = None;
                            }
                            if let Some(stream) = &mut view.stream {
                                stream.reload(&mut commands);
                            }
                            PasteState::Saved(result)
                        }
                        Err(error) => PasteState::Error(Diagnostic::from_error(error)),
                    };
                }
            }
            _ => (),
        }
    }
}
