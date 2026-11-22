use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    mesh::Indices,
    prelude::*,
    render::render_resource::{Extent3d, PrimitiveTopology, TextureDimension, TextureFormat},
};

use crate::world::{BlockState, Chunk, Section};

const FIRST_SECTION: i32 = -4;
const SECTION_COUNT: usize = 24;
const TILE_SIZE: u32 = 16;
const ATLAS_COLUMNS: u32 = 8;
const ATLAS_ROWS: u32 = 2;

pub fn terrain_material(images: &mut Assets<Image>) -> StandardMaterial {
    let width = TILE_SIZE * ATLAS_COLUMNS;
    let height = TILE_SIZE * ATLAS_ROWS;
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let tile = Tile::ALL[(x / TILE_SIZE + y / TILE_SIZE * ATLAS_COLUMNS) as usize];
            data.extend(tile.pixel(x % TILE_SIZE, y % TILE_SIZE));
        }
    }
    let mut atlas = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    atlas.sampler = ImageSampler::nearest();
    StandardMaterial {
        base_color_texture: Some(images.add(atlas)),
        alpha_mode: AlphaMode::Mask(0.5),
        perceptual_roughness: 1.0,
        reflectance: 0.0,
        ..default()
    }
}

#[derive(Clone, Copy)]
enum Tile {
    Grain,
    Rock,
    Grass,
    GrassEdge,
    Dirt,
    Water,
    Bark,
    Rings,
    Planks,
    Leaves,
    GrassBlades,
    Flower,
    Glass,
    Bricks,
    Cobble,
    Sand,
}

impl Tile {
    const ALL: [Self; 16] = [
        Self::Grain,
        Self::Rock,
        Self::Grass,
        Self::GrassEdge,
        Self::Dirt,
        Self::Water,
        Self::Bark,
        Self::Rings,
        Self::Planks,
        Self::Leaves,
        Self::GrassBlades,
        Self::Flower,
        Self::Glass,
        Self::Bricks,
        Self::Cobble,
        Self::Sand,
    ];

    fn pixel(self, x: u32, y: u32) -> [u8; 4] {
        let noise = |a: u32, b: u32| {
            let mut value = (a + 29_u32).wrapping_mul(0x8da6_b343_u32)
                ^ (b + 19_u32).wrapping_mul(0xcb1a_b31f_u32);
            value ^= value >> 13;
            value = value.wrapping_mul(0x85eb_ca6b);
            (value ^ (value >> 16)) % 32
        };
        let grain = noise(x, y) as u8;
        let gray = |value| [value, value, value, 255];
        match self {
            Self::Grain => gray(220 + grain),
            Self::Rock => gray(193 + grain + noise(x / 3, y / 3) as u8),
            Self::Grass => gray(186 + grain + noise(x / 2, y / 2) as u8),
            Self::GrassEdge if y <= 2 + noise(x, 0) % 4 => {
                [70 + grain / 2, 128 + grain, 44 + grain / 3, 255]
            }
            Self::GrassEdge => [110 + grain, 76 + grain / 2, 48 + grain / 2, 255],
            Self::Dirt => gray(184 + grain * 2),
            Self::Water => {
                let ripple = (x + y * 3 + noise(0, y) / 7) % 16;
                gray(if y % 5 == 1 && ripple < 7 {
                    253
                } else {
                    204 + grain / 2
                })
            }
            Self::Bark => gray(126 + noise(x / 2, y / 7) as u8 * 3 + grain),
            Self::Rings => {
                let distance = x.abs_diff(7).max(y.abs_diff(8));
                gray(if distance % 3 == 0 {
                    169 + grain
                } else {
                    218 + grain
                })
            }
            Self::Planks if y % 4 == 0 || (x + y / 4 * 5) % 16 == 0 => gray(151 + grain),
            Self::Planks => gray(196 + noise(x / 5, y) as u8 + grain / 2),
            Self::Leaves if noise(x / 2, y / 2) < 4 && grain < 19 => [0; 4],
            Self::Leaves => gray(178 + noise(x / 2, y / 2) as u8 + grain),
            Self::GrassBlades => {
                let main = (x as i32 - (7 + y as i32 / 7)).abs() <= 1 && y > 2;
                let left = (x as i32 - (1 + y as i32 / 3)).abs() <= 1 && y > 7;
                let right = (x as i32 - (17 - y as i32 / 3)).abs() <= 1 && y > 5;
                if main || left || right {
                    gray(185 + grain * 2)
                } else {
                    [0; 4]
                }
            }
            Self::Flower if x.abs_diff(8) + y.abs_diff(5) < 5 => gray(211 + grain),
            Self::Flower if (x == 8 && y > 5) || (y > 10 && x.abs_diff(8) < 15 - y) => {
                [50, 115 + grain * 2, 41, 255]
            }
            Self::Flower => [0; 4],
            Self::Glass if x == 0 || x == 15 || y == 0 || y == 15 => gray(210 + grain),
            Self::Glass if (x + y == 9 && y < 6) || (x + y == 12 && y < 5) => gray(245),
            Self::Glass => [0; 4],
            Self::Bricks if y % 5 == 0 || (x + y / 5 * 4) % 8 == 0 => [182, 179, 167, 255],
            Self::Bricks => gray(185 + noise((x + y / 5 * 4) / 8, y / 5) as u8 + grain),
            Self::Cobble => {
                let row = y / 6;
                let column = (x + row * 3) / 6;
                if y % 6 < 1 + noise(column, row) % 2 || (x + row * 3) % 6 == 0 {
                    gray(128 + grain)
                } else {
                    gray(196 + noise(column, row) as u8 + grain / 2)
                }
            }
            Self::Sand => gray(225 + grain / 2 + noise(x / 4, y / 4) as u8 / 3),
        }
    }

    fn uv(self, [u, v]: [f32; 2]) -> [f32; 2] {
        let index = self as u32;
        let column = index % ATLAS_COLUMNS;
        let row = index / ATLAS_COLUMNS;
        let size = TILE_SIZE as f32;
        [
            (column as f32 * size + 0.5 + u * (size - 1.0)) / (size * ATLAS_COLUMNS as f32),
            (row as f32 * size + 0.5 + v * (size - 1.0)) / (size * ATLAS_ROWS as f32),
        ]
    }
}

#[derive(Clone, Copy)]
struct Texture {
    tile: Tile,
    tint: [f32; 4],
}

impl Texture {
    fn new(tile: Tile, color: [u8; 3]) -> Self {
        let [r, g, b] = color.map(|channel| f32::from(channel) / 255.0);
        let linear = Color::srgb(r, g, b).to_linear();
        Self {
            tile,
            tint: [linear.red, linear.green, linear.blue, 1.0],
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Surface {
    Opaque,
    Foliage,
    Glass,
    Water,
    Lava,
}

#[derive(Clone, Copy)]
enum Shape {
    Empty,
    Cube,
    Octants(u8),
    Box { min: [f32; 3], max: [f32; 3] },
    Cross,
    Liquid,
}

struct Visual {
    shape: Shape,
    surface: Surface,
    side: Texture,
    top: Texture,
    bottom: Texture,
    end_grain: Option<(usize, Texture)>,
}

impl Visual {
    const EMPTY: Self = {
        const WHITE: Texture = Texture {
            tile: Tile::Grain,
            tint: [1.0; 4],
        };
        Self {
            shape: Shape::Empty,
            surface: Surface::Opaque,
            side: WHITE,
            top: WHITE,
            bottom: WHITE,
            end_grain: None,
        }
    };

    fn cube(tile: Tile, color: [u8; 3]) -> Self {
        let texture = Texture::new(tile, color);
        Self {
            shape: Shape::Cube,
            side: texture,
            top: texture,
            bottom: texture,
            ..Self::EMPTY
        }
    }

    fn texture(&self, direction: usize) -> Texture {
        if let Some((axis, texture)) = self.end_grain {
            return if direction / 2 == axis {
                texture
            } else {
                self.side
            };
        }
        match direction {
            DOWN => self.bottom,
            UP => self.top,
            _ => self.side,
        }
    }

    fn occludes(&self, surface: Surface) -> bool {
        self.surface == Surface::Opaque || self.surface == surface
    }

    fn covers_face(&self, direction: usize, surface: Surface) -> bool {
        if !self.occludes(surface) {
            return false;
        }
        match self.shape {
            Shape::Cube | Shape::Liquid => true,
            Shape::Octants(mask) => mask & FACE_OCTANTS[direction] == FACE_OCTANTS[direction],
            _ => false,
        }
    }

    fn fills_octant(&self, octant: [i32; 3], surface: Surface) -> bool {
        if !self.occludes(surface) {
            return false;
        }
        match self.shape {
            Shape::Cube | Shape::Liquid => true,
            Shape::Octants(mask) => mask & octant_bit(octant) != 0,
            _ => false,
        }
    }
}

fn block_visual(state: &BlockState) -> Visual {
    use Tile::*;

    let name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    let property = |key: &str| state.properties.get(key).map(String::as_str);
    let cube = Visual::cube;
    let mut visual = match name {
        "air" | "cave_air" | "void_air" | "structure_void" | "light" | "barrier" => Visual::EMPTY,
        "grass_block" | "podzol" | "mycelium" => {
            let top = match name {
                "podzol" => [128, 103, 63],
                "mycelium" => [136, 109, 132],
                _ => [113, 170, 69],
            };
            let mut grass = cube(Dirt, [154, 109, 72]);
            grass.top = Texture::new(Grass, top);
            grass.side = if name == "grass_block" {
                Texture::new(GrassEdge, [255; 3])
            } else {
                Texture::new(Dirt, top)
            };
            grass
        }
        "dirt" | "coarse_dirt" | "rooted_dirt" | "farmland" | "dirt_path" => cube(
            Dirt,
            if name == "dirt_path" {
                [168, 136, 77]
            } else {
                [151, 107, 70]
            },
        ),
        "stone" | "smooth_stone" | "andesite" | "polished_andesite" => cube(Rock, [150, 151, 150]),
        "granite" | "polished_granite" => cube(Rock, [179, 126, 109]),
        "diorite" | "polished_diorite" | "calcite" => cube(Rock, [208, 208, 205]),
        "bedrock" | "deepslate" | "tuff" => cube(Rock, [92, 93, 96]),
        "gravel" => cube(Dirt, [150, 143, 144]),
        "sand" | "sandstone" | "smooth_sandstone" => cube(Sand, [229, 211, 147]),
        "red_sand" | "red_sandstone" | "terracotta" => cube(Sand, [185, 105, 62]),
        "clay" => cube(Sand, [161, 168, 184]),
        "mud" | "packed_mud" => cube(Dirt, [104, 87, 84]),
        "snow" | "snow_block" | "powder_snow" | "quartz_block" => cube(Sand, [242, 250, 255]),
        "ice" | "packed_ice" | "blue_ice" => cube(Water, [159, 192, 236]),
        "water" | "bubble_column" | "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass" => {
            Visual {
                shape: Shape::Liquid,
                surface: Surface::Water,
                ..cube(Water, [71, 132, 209])
            }
        }
        "lava" => Visual {
            shape: Shape::Liquid,
            surface: Surface::Lava,
            ..cube(Water, [245, 117, 32])
        },
        "short_grass" | "tall_grass" | "fern" | "large_fern" | "dead_bush" | "sugar_cane"
        | "vine" | "wheat" | "carrots" | "potatoes" | "beetroots" => Visual {
            shape: Shape::Cross,
            ..cube(
                GrassBlades,
                if name == "dead_bush" {
                    [157, 128, 70]
                } else {
                    [118, 187, 66]
                },
            )
        },
        "dandelion" | "sunflower" => flower([249, 219, 56]),
        "poppy" | "red_tulip" | "rose_bush" => flower([231, 57, 46]),
        "azure_bluet" | "oxeye_daisy" | "white_tulip" | "lily_of_the_valley" => {
            flower([250, 246, 232])
        }
        "blue_orchid" | "cornflower" => flower([94, 150, 236]),
        "allium" | "lilac" | "pink_tulip" | "peony" | "pink_petals" => flower([224, 142, 201]),
        "orange_tulip" | "torchflower" => flower([243, 146, 57]),
        "moss_block" | "moss_carpet" => cube(Grass, [106, 152, 67]),
        "chest" | "trapped_chest" | "barrel" | "crafting_table" => cube(Planks, [172, 112, 55]),
        "torch" | "wall_torch" | "lantern" => cube(Grain, [246, 199, 105]),
        "glass" | "glass_pane" | "tinted_glass" => Visual {
            surface: Surface::Glass,
            ..cube(Glass, [192, 221, 224])
        },
        n if n.ends_with("_stained_glass") || n.ends_with("_stained_glass_pane") => Visual {
            surface: Surface::Glass,
            ..cube(Glass, dye(n).unwrap_or([193, 215, 217]))
        },
        n if n.ends_with("_leaves") => {
            let tint = match n {
                "spruce_leaves" => [78, 123, 83],
                "birch_leaves" => [124, 164, 75],
                "cherry_leaves" => [240, 160, 185],
                "dark_oak_leaves" => [71, 122, 57],
                _ => [99, 155, 64],
            };
            Visual {
                surface: Surface::Foliage,
                ..cube(Leaves, tint)
            }
        }
        n if n.ends_with("_log")
            || n.ends_with("_wood")
            || n.ends_with("_stem")
            || n.ends_with("_hyphae") =>
        {
            let (wood, bark) = wood_colors(n).unwrap_or(([181, 139, 88], [98, 77, 53]));
            let bark = if n.starts_with("stripped_") {
                wood
            } else {
                bark
            };
            let mut log = cube(Bark, bark);
            if n.ends_with("_log") || n.ends_with("_stem") {
                let axis = match property("axis") {
                    Some("x") => 0,
                    Some("z") => 2,
                    _ => 1,
                };
                log.end_grain = Some((axis, Texture::new(Rings, wood)));
            }
            log
        }
        n if n.contains("cobblestone") || n.contains("cobbled_deepslate") => {
            let color = if n.contains("deepslate") {
                [99, 99, 106]
            } else {
                [169, 168, 162]
            };
            cube(Cobble, color)
        }
        n if n == "bricks" || n.starts_with("brick_") => cube(Bricks, [188, 108, 93]),
        n if n.contains("brick") => cube(Bricks, [147, 149, 149]),
        n if n.ends_with("_ore") => cube(Rock, [123, 125, 126]),
        n if n.ends_with("_sapling") => Visual {
            shape: Shape::Cross,
            ..cube(GrassBlades, [126, 179, 84])
        },
        _ => {
            if let Some((wood, _)) = wood_colors(name) {
                cube(Planks, wood)
            } else if let Some(color) = dye(name) {
                cube(Grain, color)
            } else {
                cube(Rock, [151, 147, 157])
            }
        }
    };

    if name.ends_with("_slab") {
        visual.shape = match property("type") {
            Some("double") => Shape::Cube,
            Some("top") => Shape::Octants(0xf0),
            _ => Shape::Octants(0x0f),
        };
    } else if name.ends_with("_stairs") {
        visual.shape = Shape::Octants(stair_octants(
            property("facing"),
            property("half"),
            property("shape"),
        ));
    } else if matches!(name, "dirt_path" | "farmland" | "snow" | "moss_carpet")
        || name.ends_with("_carpet")
    {
        let height = match name {
            "snow" => {
                f32::from(
                    property("layers")
                        .and_then(|n| n.parse::<u8>().ok())
                        .unwrap_or(1)
                        .clamp(1, 8),
                ) / 8.0
            }
            "farmland" | "dirt_path" => 0.9375,
            _ => 0.0625,
        };
        visual.shape = if height == 1.0 {
            Shape::Cube
        } else {
            Shape::Box {
                min: [0.0; 3],
                max: [1.0, height, 1.0],
            }
        };
    } else if matches!(name, "chest" | "trapped_chest") {
        visual.shape = Shape::Box {
            min: [0.0625, 0.0, 0.0625],
            max: [0.9375, 0.875, 0.9375],
        };
    } else if matches!(name, "torch" | "wall_torch") {
        visual.shape = Shape::Box {
            min: [0.43, 0.0, 0.43],
            max: [0.57, 0.65, 0.57],
        };
    }

    visual
}

fn flower(color: [u8; 3]) -> Visual {
    Visual {
        shape: Shape::Cross,
        ..Visual::cube(Tile::Flower, color)
    }
}

fn wood_colors(name: &str) -> Option<([u8; 3], [u8; 3])> {
    let name = name.strip_prefix("stripped_").unwrap_or(name);
    let species = [
        ("dark_oak_", [100, 72, 42], [72, 61, 49]),
        ("oak_", [190, 155, 95], [109, 86, 56]),
        ("spruce_", [132, 95, 59], [83, 63, 46]),
        ("birch_", [224, 205, 151], [212, 208, 190]),
        ("jungle_", [177, 122, 89], [103, 94, 58]),
        ("acacia_", [184, 109, 67], [109, 105, 100]),
        ("mangrove_", [157, 83, 73], [95, 78, 59]),
        ("cherry_", [235, 174, 157], [93, 63, 74]),
        ("bamboo_", [216, 195, 105], [134, 158, 73]),
        ("crimson_", [151, 72, 111], [113, 64, 83]),
        ("warped_", [64, 151, 139], [70, 100, 101]),
    ];
    species
        .into_iter()
        .find_map(|(prefix, wood, bark)| name.starts_with(prefix).then_some((wood, bark)))
}

fn dye(name: &str) -> Option<[u8; 3]> {
    let dyes = [
        ("light_gray_", [161, 161, 153]),
        ("light_blue_", [93, 180, 222]),
        ("white_", [236, 235, 226]),
        ("orange_", [224, 136, 58]),
        ("magenta_", [192, 79, 178]),
        ("yellow_", [229, 199, 67]),
        ("lime_", [136, 189, 56]),
        ("pink_", [225, 137, 162]),
        ("gray_", [89, 91, 95]),
        ("cyan_", [47, 147, 155]),
        ("purple_", [127, 82, 167]),
        ("blue_", [73, 88, 175]),
        ("brown_", [119, 81, 56]),
        ("green_", [100, 126, 55]),
        ("red_", [174, 58, 52]),
        ("black_", [49, 48, 52]),
    ];
    dyes.into_iter()
        .find_map(|(prefix, color)| name.starts_with(prefix).then_some(color))
}

const WEST: usize = 0;
const EAST: usize = 1;
const DOWN: usize = 2;
const UP: usize = 3;
const NORTH: usize = 4;
const SOUTH: usize = 5;
const DIRECTIONS: [[i32; 3]; 6] = [
    [-1, 0, 0],
    [1, 0, 0],
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
];
const FACE_OCTANTS: [u8; 6] = [0x55, 0xaa, 0x0f, 0xf0, 0x33, 0xcc];
const CORNERS: [[[f32; 3]; 4]; 6] = [
    [
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 1.0],
        [0.0, 1.0, 0.0],
    ],
    [
        [1.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [1.0, 1.0, 1.0],
    ],
    [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
    ],
    [
        [0.0, 1.0, 1.0],
        [1.0, 1.0, 1.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ],
    [
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 1.0, 0.0],
    ],
    [
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
        [0.0, 1.0, 1.0],
    ],
];

fn stair_octants(facing: Option<&str>, half: Option<&str>, shape: Option<&str>) -> u8 {
    let (forward, left, right) = match facing {
        Some("east") => (EAST, NORTH, SOUTH),
        Some("south") => (SOUTH, EAST, WEST),
        Some("west") => (WEST, SOUTH, NORTH),
        _ => (NORTH, WEST, EAST),
    };
    let (base, step) = if half == Some("top") {
        (UP, DOWN)
    } else {
        (DOWN, UP)
    };
    let forward = FACE_OCTANTS[forward];
    let treads = match shape {
        Some("outer_left") => forward & FACE_OCTANTS[left],
        Some("outer_right") => forward & FACE_OCTANTS[right],
        Some("inner_left") => forward | FACE_OCTANTS[left],
        Some("inner_right") => forward | FACE_OCTANTS[right],
        _ => forward,
    };
    FACE_OCTANTS[base] | (FACE_OCTANTS[step] & treads)
}

fn octant_bit([x, y, z]: [i32; 3]) -> u8 {
    1 << (x + z * 2 + y * 4)
}

struct RenderSection<'a> {
    blocks: &'a [u16; 4096],
    palette: Vec<Visual>,
    opaque: bool,
}

impl<'a> RenderSection<'a> {
    fn new(section: &'a Section) -> Option<Self> {
        let palette: Vec<_> = section.palette.iter().map(block_visual).collect();
        if palette
            .iter()
            .all(|block| matches!(block.shape, Shape::Empty))
        {
            return None;
        }
        let opaque = palette
            .iter()
            .all(|block| matches!(block.shape, Shape::Cube) && block.surface == Surface::Opaque);
        Some(Self {
            blocks: &section.blocks,
            palette,
            opaque,
        })
    }

    fn block(&self, index: usize) -> &Visual {
        &self.palette[usize::from(self.blocks[index])]
    }
}

struct RenderChunk<'a> {
    sections: [Option<RenderSection<'a>>; SECTION_COUNT],
}

impl<'a> RenderChunk<'a> {
    fn new(chunk: Option<&'a Chunk>) -> Self {
        Self {
            sections: std::array::from_fn(|index| {
                let y = index as i32 + FIRST_SECTION;
                chunk?.sections.get(&y).and_then(RenderSection::new)
            }),
        }
    }

    fn section(&self, index: usize) -> Option<&RenderSection<'a>> {
        self.sections.get(index)?.as_ref()
    }
}

/// Neighbors are west, east, north and south. The mesh has local X and Z and absolute Y.
pub fn mesh_chunk(chunk: &Chunk, neighbors: [Option<&Chunk>; 4]) -> Option<Mesh> {
    let chunk = RenderChunk::new(Some(chunk));
    let [west, east, north, south] = neighbors.map(RenderChunk::new);
    let mut data = Quads::default();
    for (section_index, section) in chunk.sections.iter().enumerate() {
        let Some(section) = section else { continue };
        let outside = [
            west.section(section_index),
            east.section(section_index),
            chunk.section(section_index.wrapping_sub(1)),
            chunk.section(section_index + 1),
            north.section(section_index),
            south.section(section_index),
        ];
        if section.opaque
            && outside
                .iter()
                .all(|side| side.is_some_and(|side| side.opaque))
        {
            continue;
        }
        let base_y = (section_index as i32 + FIRST_SECTION) * 16;
        for index in 0..4096 {
            let x = index & 15;
            let z = (index >> 4) & 15;
            let y = index >> 8;
            if section.opaque
                && (1..15).contains(&x)
                && (1..15).contains(&y)
                && (1..15).contains(&z)
            {
                continue;
            }
            let block = section.block(index);
            if matches!(block.shape, Shape::Empty) {
                continue;
            }
            let origin = Vec3::new(x as f32, (base_y + y as i32) as f32, z as f32);
            let adjacent = |direction| {
                let (inside, wrapped) = match direction {
                    WEST => (x > 0, (index & !15) + (x.wrapping_sub(1) & 15)),
                    EAST => (x < 15, (index & !15) + ((x + 1) & 15)),
                    DOWN => (y > 0, (index & 255) + (y.wrapping_sub(1) & 15) * 256),
                    UP => (y < 15, (index & 255) + ((y + 1) & 15) * 256),
                    NORTH => (z > 0, (index & !240) + (z.wrapping_sub(1) & 15) * 16),
                    _ => (z < 15, (index & !240) + ((z + 1) & 15) * 16),
                };
                if inside {
                    section.block(wrapped)
                } else {
                    outside[direction].map_or(&Visual::EMPTY, |side| side.block(wrapped))
                }
            };
            data.block(origin, block, adjacent);
        }
    }
    data.into_mesh()
}

#[derive(Default)]
struct Quads {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Quads {
    fn block<'a>(&mut self, origin: Vec3, block: &Visual, adjacent: impl Fn(usize) -> &'a Visual) {
        match block.shape {
            Shape::Empty => (),
            Shape::Cube => self.box_faces(origin, block, [0.0; 3], [1.0; 3], adjacent),
            Shape::Box { min, max } => self.box_faces(origin, block, min, max, adjacent),
            Shape::Liquid => {
                let above = adjacent(UP);
                let height =
                    if matches!(above.shape, Shape::Liquid) && above.surface == block.surface {
                        1.0
                    } else {
                        0.875
                    };
                self.box_faces(origin, block, [0.0; 3], [1.0, height, 1.0], adjacent);
            }
            Shape::Octants(mask) => self.octants(origin, block, mask, adjacent),
            Shape::Cross => self.cross(origin, block.side),
        }
    }

    fn box_faces<'a>(
        &mut self,
        origin: Vec3,
        block: &Visual,
        min: [f32; 3],
        max: [f32; 3],
        adjacent: impl Fn(usize) -> &'a Visual,
    ) {
        for direction in 0..DIRECTIONS.len() {
            let axis = direction / 2;
            let on_boundary = if direction % 2 == 0 {
                min[axis] == 0.0
            } else {
                max[axis] == 1.0
            };
            if on_boundary && adjacent(direction).covers_face(direction ^ 1, block.surface) {
                continue;
            }
            self.face(origin, direction, min, max, block.texture(direction));
        }
    }

    fn octants<'a>(
        &mut self,
        origin: Vec3,
        block: &Visual,
        mask: u8,
        adjacent: impl Fn(usize) -> &'a Visual,
    ) {
        let neighbors: [&Visual; 6] = std::array::from_fn(adjacent);
        for index in 0..8 {
            if mask & (1 << index) == 0 {
                continue;
            }
            let octant = [index & 1, index >> 2, (index >> 1) & 1];
            let min = octant.map(|value| value as f32 * 0.5);
            let max = min.map(|value| value + 0.5);
            for (direction, step) in DIRECTIONS.iter().enumerate() {
                let axis = direction / 2;
                let mut next = octant;
                next[axis] += step[axis];
                if (0..2).contains(&next[axis]) {
                    if mask & octant_bit(next) != 0 {
                        continue;
                    }
                } else {
                    next[axis] = next[axis].rem_euclid(2);
                    if neighbors[direction].fills_octant(next, block.surface) {
                        continue;
                    }
                }
                self.face(origin, direction, min, max, block.texture(direction));
            }
        }
    }

    fn cross(&mut self, origin: Vec3, texture: Texture) {
        let planes = [
            [
                [0.05, 0.0, 0.05],
                [0.95, 0.0, 0.95],
                [0.95, 1.0, 0.95],
                [0.05, 1.0, 0.05],
            ],
            [
                [0.95, 0.0, 0.05],
                [0.05, 0.0, 0.95],
                [0.05, 1.0, 0.95],
                [0.95, 1.0, 0.05],
            ],
        ];
        let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for plane in planes {
            let positions = plane.map(|corner| Vec3::from(corner) + origin);
            let normal = (positions[1] - positions[0])
                .cross(positions[2] - positions[0])
                .normalize_or_zero();
            self.quad(positions, uv, normal, texture, 1.0);
            let back = [1, 0, 3, 2];
            self.quad(
                back.map(|i| positions[i]),
                back.map(|i| uv[i]),
                -normal,
                texture,
                1.0,
            );
        }
    }

    fn face(
        &mut self,
        origin: Vec3,
        direction: usize,
        min: [f32; 3],
        max: [f32; 3],
        texture: Texture,
    ) {
        let local = CORNERS[direction].map(|[x, y, z]| {
            [
                if x > 0.0 { max[0] } else { min[0] },
                if y > 0.0 { max[1] } else { min[1] },
                if z > 0.0 { max[2] } else { min[2] },
            ]
        });
        let uv = local.map(|[x, y, z]| match direction {
            WEST => [z, 1.0 - y],
            EAST => [1.0 - z, 1.0 - y],
            DOWN | UP => [x, z],
            NORTH => [1.0 - x, 1.0 - y],
            _ => [x, 1.0 - y],
        });
        let shade = match direction {
            UP => 1.0,
            DOWN => 0.68,
            WEST | EAST => 0.84,
            _ => 0.91,
        };
        self.quad(
            local.map(|corner| Vec3::from(corner) + origin),
            uv,
            Vec3::from(DIRECTIONS[direction].map(|coord| coord as f32)),
            texture,
            shade,
        );
    }

    fn quad(
        &mut self,
        positions: [Vec3; 4],
        uv: [[f32; 2]; 4],
        normal: Vec3,
        texture: Texture,
        shade: f32,
    ) {
        let start = self.positions.len() as u32;
        let [r, g, b, a] = texture.tint;
        self.positions
            .extend(positions.map(|position| position.to_array()));
        self.normals.extend([normal.to_array(); 4]);
        self.uvs.extend(uv.map(|uv| texture.tile.uv(uv)));
        self.colors
            .extend([[r * shade, g * shade, b * shade, a]; 4]);
        self.indices
            .extend([start, start + 1, start + 2, start, start + 2, start + 3]);
    }

    fn into_mesh(self) -> Option<Mesh> {
        if self.indices.is_empty() {
            return None;
        }
        Some(
            Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
            .with_inserted_indices(Indices::U32(self.indices)),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn isolated_cube(x: usize) -> Chunk {
        let mut blocks = Box::new([0; 4096]);
        blocks[x] = 1;
        let palette = ["minecraft:air", "minecraft:stone"]
            .into_iter()
            .map(|name| BlockState {
                name: name.to_owned(),
                properties: BTreeMap::new(),
            })
            .collect();
        Chunk {
            sections: BTreeMap::from([(0, Section { palette, blocks })]),
            ..default()
        }
    }

    #[test]
    fn culls_the_face_shared_by_adjacent_chunks() {
        let chunk = isolated_cube(15);
        let neighbor = isolated_cube(0);
        let face_count = |chunks| mesh_chunk(&chunk, chunks).unwrap().indices().unwrap().len() / 6;
        assert_eq!(face_count([None; 4]), 6);
        assert_eq!(face_count([None, Some(&neighbor), None, None]), 5);
    }

    #[test]
    fn cube_faces_have_outward_winding() {
        for (corners, normal) in CORNERS.into_iter().zip(DIRECTIONS) {
            let [a, b, c, _] = corners.map(Vec3::from);
            assert_eq!(
                (b - a).cross(c - a),
                Vec3::from(normal.map(|value| value as f32))
            );
        }
    }

    #[test]
    fn corner_stairs_vary_the_number_of_occupied_octants() {
        let stairs = |shape| stair_octants(Some("east"), Some("bottom"), Some(shape));
        assert_eq!(stairs("straight"), 0xaf);
        assert_eq!(stairs("outer_left"), 0x2f);
        assert_eq!(stairs("inner_left"), 0xbf);
    }
}
