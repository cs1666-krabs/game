use crate::{
    network::BINCODE_CONFIG,
    procedural_functions::{
        self, dist_to_vein, generate_perlin_noise, generate_random_cave, generate_random_vein,
        generate_random_vein_count,
    },
    save, states,
};
use bevy::prelude::*;
use iyes_loopless::prelude::*;
use std::fs::*;
use std::io::Write;

use bincode::{BorrowDecode, Decode, Encode};
use rand::Rng;

pub const CHUNK_HEIGHT: usize = 64;
pub const CHUNK_WIDTH: usize = 128;

const BASE_SEED: u64 = 82981925813;

/// Increase for smaller caves
/// Decrease for bigger caves
const PERLIN_CAVE_THRESHOLD: f32 = 1.75;

pub mod client {
    use super::*;
    pub struct WorldPlugin;

    impl Plugin for WorldPlugin {
        fn build(&self, app: &mut App) {
            // TODO: get baseline terrain from server, then insert it as a resource
            // then make a system that spawns in the entities from the resource
            app.add_enter_system(states::client::GameState::InGame, create_world)
                .add_system_set(
                    ConditionSet::new()
                        .run_in_state(states::client::GameState::InGame)
                        .with_system(f2_prints_terrain)
                        .with_system(g_deletes_random_block)
                        .into(),
                )
                .add_exit_system(states::client::GameState::InGame, destroy_world);
        }
    }
}

pub mod server {
    use crate::network;

    use super::*;

    use iyes_loopless::prelude::*;

    pub struct WorldPlugin;

    impl Plugin for WorldPlugin {
        fn build(&self, app: &mut App) {
            app.add_enter_system(states::server::GameState::Running, create_world);

            app.add_exit_system(states::server::GameState::Running, destroy_world);
        }
    }
}

pub fn create_world(mut commands: Commands) {
    info!("creating world");

    // create now, insert as resource later
    let mut terrain = Terrain::empty();

    // Generate one chunk
    // TODO: move this into terrain creation
    create_surface_chunk(&mut terrain);

    // now add as resource
    commands.insert_resource(terrain);
}

fn destroy_world(mut commands: Commands, query: Query<Entity, With<RenderedBlock>>) {
    info!("destroying world");
    // remove all block sprites
    for entity in query.iter() {
        commands.entity(entity).despawn();
    }

    commands.remove_resource::<Terrain>();
}

/// Represents all chunks in the game world
/// Should be a global resource
#[derive(Encode, Decode, Debug, PartialEq, Clone)]
pub struct Terrain {
    /// Vector of chunks, each one contains its own chunk_number
    /// TODO: potentially convert into a symbol table for faster lookups?
    pub chunks: Vec<Chunk>,
}

impl Terrain {
    /// Create a terrain with specified number of chunks
    /// Chunks contain default blocks and are numbered from 0 to len-1
    pub fn new(num_chunks: u64) -> Terrain {
        let chunks = (0..num_chunks).map(|d| Chunk::new(d)).collect();

        Terrain { chunks }
    }

    /// Creates a terrain with no chunks
    pub fn empty() -> Terrain {
        Terrain { chunks: Vec::new() }
    }
}

/// Represents a chunk of blocks; stored in the Terrain resource
/// TODO: maybe custom bitpack for Encode and Decode?
#[derive(Encode, Decode, Debug, PartialEq, Clone)]
pub struct Chunk {
    /// 2D array [x, y]
    pub blocks: [[Option<Block>; CHUNK_WIDTH]; CHUNK_HEIGHT],
    pub rendered: bool,
    /// starting row for blocks is chunk_number * CHUNK_HEIGHT
    pub chunk_number: u64,
}

impl Chunk {
    pub fn new(depth: u64) -> Self {
        // start with empty chunk
        let mut c = Chunk {
            blocks: [[None; CHUNK_WIDTH]; CHUNK_HEIGHT],
            chunk_number: depth,
            rendered: false,
        };
        let mut tree = true;

        // generate chunks for current and previous chunk
        let mut veins = Vec::new();
        if depth > 0 {
            for vein_number in 0..generate_random_vein_count(BASE_SEED, depth - 1) {
                veins.push(Vein::new(depth, vein_number));
            }
        }
        for vein_number in 0..generate_random_vein_count(BASE_SEED, depth) {
            veins.push(Vein::new(depth, vein_number));
        }

        // get prev biome
        let mut prev_biome_search: Option<BiomeType> = None;

        if depth > 0 {
            let mut curr_search_depth = depth - 1;

            while prev_biome_search.is_none() {
                prev_biome_search = if depth > 0 {
                    procedural_functions::generate_chunk_biome_change(BASE_SEED, curr_search_depth)
                } else {
                    Some(BiomeType::Sand)
                };
                info! {
                    "Trying to find biome for {} - currently {:?}",
                    curr_search_depth,
                    prev_biome_search
                }
                if curr_search_depth == 0 {
                    break; // can't put >= 0 in the while condititon since it's unsigned and that'll always be true
                }
                curr_search_depth -= 1;
            }
        }

        let prev_biome = prev_biome_search.unwrap_or(BiomeType::Sand);

        // Determine biome of chunk and whether there will be a biome change
        let biome_change = procedural_functions::generate_chunk_biome_change(BASE_SEED, depth)
            .unwrap_or(prev_biome);

        let average_biome_change_depth = procedural_functions::generate_random_values(
            procedural_functions::generate_seed(BASE_SEED, vec![depth, 432]),
            1,
            3,
            10,
        )[0] as usize;

        let biome_change_depths = procedural_functions::generate_random_values(
            procedural_functions::generate_seed(BASE_SEED, vec![depth, 234]),
            64, // interpolate between 64 values
            average_biome_change_depth - 2,
            average_biome_change_depth + 2, // 5 block range
        );

        info!(
            "Chunk {} has biome change from {:?} to {:?} between {} and {}",
            depth,
            prev_biome,
            biome_change,
            average_biome_change_depth + 2,
            average_biome_change_depth - 2,
        );

        let perlin_vals = generate_perlin_noise(depth, BASE_SEED);

        // Loop through chunk, filling in where blocks should be
        for x in 0..CHUNK_WIDTH {
            for y in 0..CHUNK_HEIGHT {
                let biome_change_ypos =
                    procedural_functions::slice_pos_x(x, &biome_change_depths).round() as usize - 1;

                let mut block_type = if y >= biome_change_ypos {
                    biome_change.primary_block()
                } else {
                    prev_biome.primary_block()
                };

                // Check if this is within the bounds of an ore vein
                for vein in &veins {
                    // Only look at veins originating in previous or current chunk
                    if depth > 0
                        && ((vein.chunk_number == depth - 1) || (vein.chunk_number == depth))
                    {
                        let y_offset = if depth > vein.chunk_number {
                            CHUNK_HEIGHT
                        } else {
                            0
                        };

                        let dist = dist_to_vein(vein, x as f32, (y + y_offset) as f32);

                        if dist < (vein.thickness_sq / 2.).into() {
                            /* info!(
                                "Block at chunk {} {},{} in vein from {},{} to {},{} ({})",
                                depth,
                                x,
                                y,
                                vein.start_x,
                                vein.start_y,
                                vein.end_x,
                                vein.end_y,
                                dist
                            ); */
                            block_type = if y >= biome_change_ypos {
                                biome_change.ore_block()
                            } else {
                                prev_biome.ore_block()
                            };
                        }
                    }
                }

                //Add Cave Functionality
                if perlin_vals[y][x] > PERLIN_CAVE_THRESHOLD {
                    block_type = BlockType::CaveVoid;
                }

                if block_type != BlockType::CaveVoid {
                    c.blocks[y][x] = Some(Block {
                        block_type,
                        entity: None,
                    });
                } else {
                    let primary_block_type = if y >= biome_change_ypos {
                        biome_change.primary_block()
                    } else {
                        prev_biome.primary_block()
                    };
                    //Checks if you can make trees, if there is room for a tree, and the block it would place a tree is the current biome primary block
                    if tree
                        && y > 4
                        && y < CHUNK_HEIGHT - 1
                        && x > 4
                        && c.blocks[y + 1][x - 2] != None
                        && c.blocks[y + 1][x - 2].unwrap().block_type == primary_block_type
                    {
                        //sees how tall it can make the tree
                        let mut max = 0;
                        for height in (0..=y).rev() {
                            if c.blocks[height][x - 2] != None {
                                max = height;
                                break;
                            }
                        }
                        if y - max > 2 {
                            //Randomizes the height of the tree
                            let random_height = procedural_functions::generate_random_values(
                                BASE_SEED + x as u64, //adds x to make it more random if it has the same max and current y position
                                2,
                                max,
                                y,
                            );
                            max = *random_height.get(0).unwrap() as usize;
                        }
                        if y - max > 2 && structure_fit(c.blocks, x, max) {
                            // 02220
                            // 02120
                            // 00100
                            // 00100
                            //Creates the trunk
                            for height in (max + 1..=y).rev() {
                                c.blocks[height][x - 2] = Some(Block {
                                    block_type: BlockType::Trunk,
                                    entity: None,
                                });
                            }
                            //Creates the Leaves
                            c.blocks[max + 1][x - 1] = Some(Block {
                                block_type: BlockType::Leaves,
                                entity: None,
                            });
                            c.blocks[max + 1][x - 2] = Some(Block {
                                block_type: BlockType::Leaves,
                                entity: None,
                            });
                            c.blocks[max + 1][x - 3] = Some(Block {
                                block_type: BlockType::Leaves,
                                entity: None,
                            });
                            c.blocks[max + 2][x - 1] = Some(Block {
                                block_type: BlockType::Leaves,
                                entity: None,
                            });
                            c.blocks[max + 2][x - 3] = Some(Block {
                                block_type: BlockType::Leaves,
                                entity: None,
                            });
                        // tree=false;
                        } else {
                            c.blocks[y][x] = None;
                        }
                    } else {
                        c.blocks[y][x] = None;
                    }
                }
            }
        }

        return c;
    }

    pub fn new_surface() -> Self {
        // Create surface chunk with perlin slice functions

        let mut c = Chunk {
            blocks: [[None; CHUNK_WIDTH]; CHUNK_HEIGHT],
            chunk_number: 0,
            rendered: false,
        };

        let random_vals = procedural_functions::generate_random_values(
            BASE_SEED, //Use hard-coded seed for now
            16,        //16 random values, so 16 points to interpolate between
            3, 16, //Peaks as high as 16 blocks
        );
        let random_sand_depths = procedural_functions::generate_random_values(
            BASE_SEED, //Use hard-coded seed for now
            32,        //32 random values, so 32 points to interpolate between
            16, 31, //Peaks as high as 16 blocks
        );
        let random_trees = procedural_functions::generate_random_values(
            BASE_SEED, //Use hard-coded seed for now
            CHUNK_WIDTH,
            0,
            CHUNK_WIDTH / 8,
        );

        let octave2 = procedural_functions::perlin_slice(BASE_SEED + 25, 32, CHUNK_WIDTH, 8);

        // generate chunks for chunk
        let mut veins = Vec::new();
        for vein_number in 0..generate_random_vein_count(BASE_SEED, 0) {
            veins.push(Vein::new(0, vein_number));
        }

        // Loop through chunk, filling in where blocks should be
        for x in 0..CHUNK_WIDTH {
            let hill_top = (procedural_functions::slice_pos_x(x, &random_vals).round() as i32
                + octave2[x]) as usize
                - 1;
            let sand_depth =
                procedural_functions::slice_pos_x(x, &random_sand_depths).round() as usize - 1;

            if random_trees[x] == 1 {
                let block_type = BlockType::PalmTreeBlock;

                c.blocks[hill_top - 1][x] = Some(Block {
                    block_type,
                    entity: None,
                });
            }
            for y in hill_top..CHUNK_HEIGHT {
                let mut block_type = if y <= sand_depth {
                    BiomeType::Sand.primary_block()
                } else {
                    BiomeType::Sedimentary.primary_block()
                };

                // Check if this is within the bounds of an ore vein
                for vein in &veins {
                    // Only look at veins originating in previous or current chunk
                    if vein.chunk_number == 0 {
                        let dist = dist_to_vein(vein, x as f32, y as f32);

                        if dist < (vein.thickness_sq / 2.).into() {
                            // info!(
                            //     "Block at chunk 0 {},{} in vein from {},{} to {},{} ({})",
                            //     x, y, vein.start_x, vein.start_y, vein.end_x, vein.end_y, dist
                            // );
                            block_type = if y <= sand_depth {
                                BiomeType::Sand.ore_block()
                            } else {
                                BiomeType::Sedimentary.ore_block()
                            };
                        }
                    }
                }

                c.blocks[y][x] = Some(Block {
                    block_type,
                    entity: None,
                });
            }
        }

        return c;
    }
}
fn structure_fit(blocks: [[Option<Block>; CHUNK_WIDTH]; CHUNK_HEIGHT], x: usize, y: usize) -> bool {
    if x > 4 && x < CHUNK_WIDTH {
        if blocks[y][x - 3] == None
            && blocks[y][x - 1] == None
            && blocks[y + 1][x - 1] == None
            && blocks[y + 1][x - 3] == None
            && blocks[y + 2][x - 1] == None
            && blocks[y + 2][x - 3] == None
        {
            return true;
        }
    }
    return false;
}

#[derive(Encode, Decode, Debug, PartialEq, Clone)]
pub enum OreType {
    Primary,
}

/// Represents an ore vein; stored in the Terrain resource
#[derive(Encode, Decode, Debug, PartialEq, Clone)]
pub struct Vein {
    pub ore_type: OreType,
    pub chunk_number: u64,
    pub start_x: usize,
    pub start_y: usize,
    pub end_x: i16, // i16 because they can hypothetically be negative - which won't break anything
    pub end_y: i16,
    pub thickness_sq: f32, // squared thickness - so we don't need to do square roots
}

impl Vein {
    pub fn new(chunk_number: u64, vein_number: u64) -> Self {
        // Hard-coded seed for now
        generate_random_vein(BASE_SEED, chunk_number, vein_number)
    }
}

#[derive(Encode, Decode, Debug, PartialEq, Clone)]
pub struct Cave {
    pub block_type: BlockType,
    pub chunk_number: u64,
    pub cave_map: [[f32; CHUNK_WIDTH]; CHUNK_HEIGHT],
}

impl Cave {
    pub fn new(chunk_number: u64) -> Self {
        generate_random_cave(BASE_SEED, chunk_number)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BiomeType {
    // if adding to this, also update Distribution in procedural_functions
    Sand,
    Sedimentary,
    Basalt,
    Felsic,
    Mafic,
    Ultramafic,
}

impl BiomeType {
    pub fn primary_block(&self) -> BlockType {
        match self {
            Self::Sand => BlockType::Sand,
            Self::Sedimentary => BlockType::Limestone,
            Self::Basalt => BlockType::Basalt,
            Self::Felsic => BlockType::Granite,
            Self::Mafic => BlockType::Diabase,
            Self::Ultramafic => BlockType::Gabbro,
        }
    }
    pub fn ore_block(&self) -> BlockType {
        match self {
            Self::Sand => BlockType::Clay,
            Self::Sedimentary => BlockType::Coal,
            Self::Basalt => BlockType::Iron,
            Self::Felsic => BlockType::Quartz,
            Self::Mafic => BlockType::Labradorite,
            Self::Ultramafic => BlockType::Peridot,
        }
    }
}

/// _Not_ a component; stored in a Chunk
#[derive(Copy, Clone, Debug)]
pub struct Block {
    /// What kind of block is this
    pub block_type: BlockType,
    /// The sprite entity that renders this block
    pub entity: Option<Entity>,
}

impl Block {
    /// Easily create a block without an Entity
    fn new(block_type: BlockType) -> Block {
        Block {
            block_type,
            entity: None,
        }
    }
}

// simple comparison, useful for testing
impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        // ignore entity differences
        self.block_type == other.block_type
    }
}

// only encode/decode the block type, not the entity
impl Encode for Block {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        bincode::Encode::encode(&self.block_type, encoder)?;
        Ok(())
    }
}

impl Decode for Block {
    fn decode<D: bincode::de::Decoder>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        Ok(Self {
            block_type: bincode::Decode::decode(decoder)?,
            entity: None,
        })
    }
}

impl<'de> bincode::BorrowDecode<'de> for Block {
    fn borrow_decode<D: bincode::de::BorrowDecoder<'de>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        Ok(Self {
            block_type: bincode::BorrowDecode::borrow_decode(decoder)?,
            entity: None,
        })
    }
}

/// Marker component for tagging sprite entity for block
#[derive(Component)]
pub struct RenderedBlock;

/// A distinct type of block, with its own texture
#[derive(Copy, Clone, Debug, Encode, Decode, PartialEq)]
pub enum BlockType {
    Sand, // primary blocks
    Limestone,
    Basalt,
    Granite,
    Diabase,
    Gabbro,
    Clay, // "ores"
    Coal,
    Iron,
    Quartz,
    Labradorite,
    Peridot,
    CaveVoid,
    PalmTreeBlock,
    Leaves,
    Trunk,
}

impl BlockType {
    /// Return the file path for the image that should be displayed for this block
    const fn image_file_path(&self) -> &str {
        match self {
            BlockType::Sand => "Sand.png",
            BlockType::Limestone => "Limestone.png",
            BlockType::Basalt => "Basalt.png",
            BlockType::Granite => "Granite.png",
            BlockType::Diabase => "Diabase.png",
            BlockType::Gabbro => "Gabbro.png",
            BlockType::Clay => "Clay.png",
            BlockType::Coal => "Coal.png",
            BlockType::Iron => "Iron.png",
            BlockType::Quartz => "Quartz.png",
            BlockType::Labradorite => "Labradorite.png",
            BlockType::Peridot => "Peridot.png",
            BlockType::CaveVoid => "",
            BlockType::PalmTreeBlock => "PalmTreeBlock.png",
            BlockType::Leaves => "Leaves.png",
            BlockType::Trunk => "Trunk.png",
        }
    }
}

/// Create all blocks in chunk as actual entities (and store references to entity in chunk.blocks)
pub fn spawn_chunk(
    chunk_number: u64,
    commands: &mut Commands,
    assets: &Res<AssetServer>,
    terrain: &mut Terrain,
) {
    let mut chunk = Chunk::new(chunk_number);
    //Calls function to loop through and create the entities and render them
    render_chunk(chunk_number, commands, assets, &mut chunk);
    // add the chunk to our terrain resource
    terrain.chunks.push(chunk);
}

pub fn render_chunk(
    chunk_number: u64,
    commands: &mut Commands,
    assets: &Res<AssetServer>,
    chunk: &mut Chunk,
) {
    //spawns each entity and asigns it
    chunk.rendered = true;
    for x in 0..CHUNK_WIDTH {
        for y in 0..CHUNK_HEIGHT {
            let block_opt = &mut chunk.blocks[y][x];

            // if there is a block at this location
            if let Some(block) = block_opt {
                // spawn in the sprite for the block
                let entity = commands
                    .spawn()
                    .insert_bundle(SpriteBundle {
                        texture: assets.load(block.block_type.image_file_path()),
                        transform: Transform {
                            translation: Vec3::from_array([
                                to_world_point_x(x),
                                to_world_point_y(y, chunk_number),
                                1.,
                            ]),
                            ..default()
                        },
                        ..default()
                    })
                    .insert(RenderedBlock)
                    .id();

                // link the entity to the block
                block.entity = Option::Some(entity);
            }
            // else there is no block and we don't have to spawn any sprite
        }
    }
}

pub fn derender_chunk(commands: &mut Commands, chunk: &mut Chunk) {
    //Despawns each entity and un asigns them
    chunk.rendered = false;
    for x in 0..CHUNK_WIDTH {
        for y in 0..CHUNK_HEIGHT {
            let block_opt = &mut chunk.blocks[y][x];
            if let Some(block) = block_opt {
                match block.entity {
                    Some(entity) => {
                        commands.entity(entity).despawn();
                    }
                    None => {}
                };
                block.entity = None;
            }
        }
    }
}

/// Create all blocks in surface chunk as actual entities (and store references to entity in chunk.blocks)
pub fn create_surface_chunk(terrain: &mut Terrain) {
    // chunk will get rendered by client
    let chunk = Chunk::new_surface();

    terrain.chunks.push(chunk);
}

#[derive(Debug)]
pub enum DestroyBlockError {
    /// Tried to search past array index in X direction
    /// TODO: make this compile-time error
    InvalidX,
    /// Corresponding chunk location is not loaded (outside Y)
    ChunkNotLoaded,
    /// Block data at the location is empty (block doesn't exist!)
    BlockDoesntExist,
}

/// Destroy a block at a global position
pub fn destroy_block(
    x: usize,
    y: usize,
    commands: &mut Commands,
    terrain: &mut Terrain,
) -> Result<Block, DestroyBlockError> {
    let chunk_number = y / CHUNK_HEIGHT;
    let block_y_in_chunk = y % CHUNK_HEIGHT;

    // make sure our x is in range
    // TODO: do this in a const fashion?
    if x >= CHUNK_WIDTH {
        return Err(DestroyBlockError::InvalidX);
    }

    // find if we have the chunk in our terrain
    for chunk in &mut terrain.chunks {
        if chunk.chunk_number == (chunk_number as u64) {
            // we have found our chunk
            let block_opt = &mut chunk.blocks[block_y_in_chunk][x];

            match block_opt {
                Some(block) => {
                    match block.entity {
                        Some(entity) => {
                            // info!("despawning sprite for block at ({}, {})", x, y);
                            commands.entity(entity).despawn();
                        }
                        None => {
                            warn!("block at ({}, {}) exists but had no entity attached!", x, y);
                        }
                    };

                    // unlink entity
                    block.entity = None;

                    // clone block data so we can give it to the caller
                    let clone = block.clone();

                    // remove the block from our data array
                    // original block is dropped here
                    *block_opt = None;

                    // give the clone back to the caller
                    // TODO: maybe give a different data type?
                    return Ok(clone);
                }
                None => {
                    // warn!("no block exists at ({}, {})", x, y);
                    return Err(DestroyBlockError::BlockDoesntExist);
                }
            }
        }
    }

    Err(DestroyBlockError::ChunkNotLoaded)
}

pub fn block_exists(x: usize, y: usize, terrain: &mut Terrain) -> bool {
    let chunk_number = y / CHUNK_HEIGHT;
    let block_y_in_chunk = y % CHUNK_HEIGHT;

    // make sure our x is in range
    // TODO: do this in a const fashion?
    if x >= CHUNK_WIDTH {
        return false;
    }

    // find if we have the chunk in our terrain
    for chunk in &mut terrain.chunks {
        if chunk.chunk_number == (chunk_number as u64) {
            // we have found our chunk
            let block_opt = &mut chunk.blocks[block_y_in_chunk][x];

            match block_opt {
                Some(block) => {
                    match block.entity {
                        Some(_entity) => {
                            return true;
                        }
                        None => {
                            warn!("block at ({}, {}) exists but had no entity attached!", x, y);
                            return true;
                        }
                    };
                }
                None => {
                    return false;
                }
            }
        }
    }

    return false;
}

pub fn to_world_point_x(x: usize) -> f32 {
    return (x as f32) * 32.;
}
pub fn to_world_point_y(y: usize, chunk_number: u64) -> f32 {
    return -(y as f32 + chunk_number as f32 * CHUNK_HEIGHT as f32) * 32.;
}

fn print_encoding_sizes() {
    match bincode::encode_to_vec(Block::new(BlockType::Limestone), BINCODE_CONFIG) {
        Ok(block) => info!("a sandstone block is {} byte(s)", block.len()),
        Err(e) => error!("unable to encode block: {}", e),
    }

    match bincode::encode_to_vec(Chunk::new(0), BINCODE_CONFIG) {
        Ok(chunk) => info!("a default chunk is {} bytes", chunk.len()),
        Err(e) => error!("unable to encode chunk: {}", e),
    }

    match bincode::encode_to_vec(Terrain::new(1), BINCODE_CONFIG) {
        Ok(terrain) => info!("a default terrain with 1 chunk is {} bytes", terrain.len()),
        Err(e) => error!("unable to encode terrina: {}", e),
    }
}

/// Make the F2 key dump the encoded terrain
fn f2_prints_terrain(input: Res<Input<KeyCode>>, terrain: Res<Terrain>) {
    // return early if F2 was not just pressed
    if !input.just_pressed(KeyCode::F2) {
        return;
    }

    print_encoding_sizes();

    // try to encode, allocating a vec
    // in a real packet, we should use a pre-allocated array and encode into its slice
    match bincode::encode_to_vec(terrain.as_ref(), BINCODE_CONFIG) {
        Ok(encoded_vec) => {
            // we have successfully encoded
            let mut encoded_str = String::new();
            // print one long string of bytes, hex representation
            for byte in &encoded_vec {
                encoded_str.push_str(&format!("{:02x} ", byte));
            }
            info!(
                "current terrain is {} bytes: {}",
                encoded_vec.len(),
                encoded_str
            );
        }
        Err(e) => {
            // unable to encode
            error!("unable to encode terrain, {}", e);
        }
    }
}

// Load world from vec (assumes terrain is cleared)
pub fn spawn_sprites_from_terrain(
    commands: &mut Commands,
    assets: &AssetServer,
    terrain: &mut Terrain,
) {
    for chunk in &mut terrain.chunks {
        for x in 0..CHUNK_WIDTH {
            for y in 0..CHUNK_HEIGHT {
                let block_opt = &mut chunk.blocks[y][x];
                // if there is a block at this location
                if let Some(block) = block_opt {
                    // spawn in the sprite for the block
                    let entity = commands
                        .spawn()
                        .insert_bundle(SpriteBundle {
                            texture: assets.load(block.block_type.image_file_path()),
                            transform: Transform {
                                translation: Vec3::from_array([
                                    to_world_point_x(x),
                                    to_world_point_y(y, chunk.chunk_number),
                                    1.,
                                ]),
                                ..default()
                            },
                            ..default()
                        })
                        .insert(RenderedBlock)
                        .id();

                    // link the entity to the block
                    block.entity = Option::Some(entity);
                }
            }
        }
    }
}

/// Make the G key delete a random block in the first chunk
fn g_deletes_random_block(
    input: Res<Input<KeyCode>>,
    mut commands: Commands,
    mut terrain: ResMut<Terrain>,
) {
    // return early if g was not just pressed
    if !input.pressed(KeyCode::G) {
        return;
    }

    let (x, y) = (
        rand::thread_rng().gen_range(0..CHUNK_WIDTH),
        rand::thread_rng().gen_range(0..CHUNK_HEIGHT),
    );

    // don't care about result here
    let _res = destroy_block(x, y, &mut commands, &mut terrain);
}

/// unit tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_block() {
        let original = Block::new(BlockType::Limestone);
        let encoded = bincode::encode_to_vec(original, BINCODE_CONFIG).unwrap();
        let decoded: Block = bincode::decode_from_slice(&encoded, BINCODE_CONFIG)
            .unwrap()
            .0;
        assert_eq!(original, decoded);
    }

    #[test]
    fn encode_decode_chunk() {
        let original = {
            let mut chunk = Chunk::new(0);
            // change some block
            chunk.blocks[1][1] = Some(Block::new(BlockType::Limestone));
            chunk
        };
        let encoded = bincode::encode_to_vec(&original, BINCODE_CONFIG).unwrap();
        let decoded: Chunk = bincode::decode_from_slice(&encoded, BINCODE_CONFIG)
            .unwrap()
            .0;
        assert_eq!(original, decoded);
    }

    #[test]
    fn encode_decode_terrain() {
        let original = {
            let mut terrain = Terrain::new(2);
            // change some block
            terrain.chunks[1].blocks[1][1] = Some(Block::new(BlockType::Limestone));
            terrain
        };
        let encoded = bincode::encode_to_vec(&original, BINCODE_CONFIG).unwrap();
        let decoded: Terrain = bincode::decode_from_slice(&encoded, BINCODE_CONFIG)
            .unwrap()
            .0;
        assert_eq!(original, decoded);
    }

    #[test]
    fn size_sanity_check() {
        let block_size = bincode::encode_to_vec(Block::new(BlockType::Limestone), BINCODE_CONFIG)
            .unwrap()
            .len();
        let chunk_size = bincode::encode_to_vec(Chunk::new(0), BINCODE_CONFIG)
            .unwrap()
            .len();
        let terrain_size = bincode::encode_to_vec(Terrain::new(1), BINCODE_CONFIG)
            .unwrap()
            .len();
        assert!(terrain_size > chunk_size);
        assert!(terrain_size > block_size);
        assert!(chunk_size > block_size);
    }
}
