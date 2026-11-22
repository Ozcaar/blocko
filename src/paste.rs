use std::sync::Arc;

use anyhow::Result;
use bevy::{
    prelude::*,
    tasks::{IoTaskPool, Task, futures::check_ready},
};

pub use crate::world_write::{PastePlan, PasteResult, PasteSummary};
use crate::{editor::View, world_write};

#[derive(Default)]
pub enum PasteState {
    #[default]
    Idle,
    Preparing(Task<Result<PastePlan>>),
    Ready(PastePlan),
    Saving(Task<Result<PasteResult>>),
    Saved(PasteResult),
    Error(String),
}

impl PasteState {
    pub fn locks_preview(&self) -> bool {
        matches!(self, Self::Preparing(_) | Self::Ready(_) | Self::Saving(_))
    }

    pub fn is_saving(&self) -> bool {
        matches!(self, Self::Saving(_))
    }
}

#[derive(Clone, Copy)]
pub enum PasteAction {
    Prepare,
    Confirm,
    Discard,
}

pub fn apply_action(view: &mut View, action: PasteAction) {
    match action {
        PasteAction::Discard if !view.paste.is_saving() => {
            view.paste = PasteState::Idle;
        }
        PasteAction::Prepare if !view.paste.locks_preview() && view.pending.is_none() => {
            let (Some(preview), Some(stream)) = (view.preview.as_mut(), view.stream.as_ref())
            else {
                return;
            };
            preview.moving = false;
            preview.notice = None;
            view.selection.picking = None;
            view.selection.pick_b_after_a = false;
            let preview = preview.clone();
            let world = stream.source();
            view.paste = PasteState::Preparing(
                IoTaskPool::get().spawn(async move { world_write::prepare(world, preview) }),
            );
        }
        PasteAction::Confirm if matches!(view.paste, PasteState::Ready(_)) => {
            let PasteState::Ready(plan) = std::mem::take(&mut view.paste) else {
                return;
            };
            if view.pending.is_some()
                || !view
                    .stream
                    .as_ref()
                    .is_some_and(|stream| Arc::ptr_eq(&stream.source(), &plan.world))
                || !view
                    .preview
                    .as_ref()
                    .is_some_and(|preview| preview.same_mask(&plan.preview))
            {
                view.paste = PasteState::Error(
                    "Cambió el destino o la vista previa. Prepara de nuevo el pegado".into(),
                );
                return;
            }
            view.paste =
                PasteState::Saving(IoTaskPool::get().spawn(async move { world_write::save(plan) }));
        }
        _ => {}
    }
}

pub fn finish(mut commands: Commands, mut views: Query<&mut View>) {
    for mut view in &mut views {
        match &mut view.paste {
            PasteState::Preparing(task) => {
                if let Some(result) = check_ready(task) {
                    view.paste = match result {
                        Ok(plan) => PasteState::Ready(plan),
                        Err(error) => PasteState::Error(format!("{error:#}")),
                    };
                }
            }
            PasteState::Saving(task) => {
                if let Some(result) = check_ready(task) {
                    view.paste = match result {
                        Ok(result) => {
                            view.preview = None;
                            if let Some(stream) = &mut view.stream {
                                stream.reload(&mut commands);
                            }
                            PasteState::Saved(result)
                        }
                        Err(error) => PasteState::Error(format!("{error:#}")),
                    };
                }
            }
            _ => (),
        }
    }
}
