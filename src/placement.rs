use std::sync::Arc;

use crate::{
    structure::{MAX_HORIZONTAL, MAX_Y, MIN_HORIZONTAL, MIN_Y, Structure, Volume},
    world::BlockState,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AirMode {
    Include,
    #[default]
    Ignore,
}

#[derive(Clone)]
pub struct Placement {
    pub source: Arc<Structure>,
    pub mode: AirMode,
    pub moving: bool,
    pub notice: Option<String>,
    origin: [i32; 3],
}

impl Placement {
    pub fn new(source: Arc<Structure>, suggested_origin: [i32; 3]) -> Self {
        let mut placement = Self {
            source,
            mode: AirMode::default(),
            moving: false,
            notice: None,
            origin: [0; 3],
        };
        placement.set_origin(suggested_origin);
        placement
    }

    pub fn origin(&self) -> [i32; 3] {
        self.origin
    }

    pub fn limits(&self) -> [(i32, i32); 3] {
        let [x, y, z] = self
            .source
            .volume
            .dimensions()
            .map(|length| length as i32 - 1);
        [
            (MIN_HORIZONTAL, MAX_HORIZONTAL - x),
            (MIN_Y, MAX_Y - y),
            (MIN_HORIZONTAL, MAX_HORIZONTAL - z),
        ]
    }

    pub fn set_origin(&mut self, origin: [i32; 3]) {
        let limits = self.limits();
        self.origin =
            std::array::from_fn(|axis| origin[axis].clamp(limits[axis].0, limits[axis].1));
    }

    pub fn volume(&self) -> Volume {
        let sides = self.source.volume.dimensions();
        Volume {
            min: self.origin,
            max: std::array::from_fn(|axis| self.origin[axis] + sides[axis] as i32 - 1),
        }
    }

    pub fn writes_block(&self, [x, y, z]: [i32; 3]) -> Option<&BlockState> {
        let [ox, oy, oz] = self.origin;
        let block = self.source.block_at([
            x.checked_sub(ox)? as u32,
            y.checked_sub(oy)? as u32,
            z.checked_sub(oz)? as u32,
        ])?;
        (self.mode == AirMode::Include || !block.is_air()).then_some(block)
    }

    pub fn same_mask(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
            && self.origin == other.origin
            && self.mode == other.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Chunk, Section};
    use std::collections::BTreeMap;

    #[test]
    fn pastes_from_the_minimum_corner_and_switches_what_air_replaces() -> anyhow::Result<()> {
        let source = Arc::new(Structure::capture(
            "Origen".into(),
            Volume::between([0, -1, 0], [2, -1, 0]),
            |_| {
                let mut blocks = Box::new([0; 4096]);
                blocks[15 * 256] = 1;
                blocks[15 * 256 + 1] = 2;
                Ok(Some(Chunk {
                    sections: BTreeMap::from([(
                        -1,
                        Section {
                            palette: ["minecraft:air", "minecraft:stone", "minecraft:cave_air"]
                                .map(|name| BlockState {
                                    name: name.into(),
                                    properties: BTreeMap::new(),
                                })
                                .into(),
                            blocks,
                        },
                    )]),
                    ..Default::default()
                }))
            },
        )?);
        let mut placement = Placement::new(source.clone(), [MAX_HORIZONTAL, MAX_Y, MIN_HORIZONTAL]);
        let x = MAX_HORIZONTAL - 2;
        assert_eq!(placement.origin(), [x, MAX_Y, MIN_HORIZONTAL]);
        assert_eq!(
            placement.volume().max,
            [MAX_HORIZONTAL, MAX_Y, MIN_HORIZONTAL]
        );
        assert_eq!(
            placement
                .writes_block([x, MAX_Y, MIN_HORIZONTAL])
                .unwrap()
                .name,
            "minecraft:stone"
        );
        assert!(
            placement
                .writes_block([x + 1, MAX_Y, MIN_HORIZONTAL])
                .is_none()
        );
        assert!(
            placement
                .writes_block([x + 2, MAX_Y, MIN_HORIZONTAL])
                .is_none()
        );
        placement.mode = AirMode::Include;
        assert_eq!(
            placement
                .writes_block([x + 1, MAX_Y, MIN_HORIZONTAL])
                .unwrap()
                .name,
            "minecraft:cave_air"
        );
        assert_eq!(
            placement
                .writes_block([x + 2, MAX_Y, MIN_HORIZONTAL])
                .unwrap()
                .name,
            "minecraft:air"
        );
        assert!(
            placement
                .writes_block([x, MAX_Y - 1, MIN_HORIZONTAL])
                .is_none()
        );
        assert!(Arc::ptr_eq(&placement.source, &source));
        Ok(())
    }
}
