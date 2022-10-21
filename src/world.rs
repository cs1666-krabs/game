use bevy::prelude::*;
use std::fs::*;
use std::io::Write;

use crate::{states, procedural_functions};


use bincode::{Decode, Encode};
use rand::Rng;

pub const CHUNK_HEIGHT: usize = 32;
pub const CHUNK_WIDTH: usize = 128;

/// This is the bincode config that we should use everywhere
/// TODO: move to a better location
const BINCODE_CONFIG: bincode::config::Configuration = bincode::config::standard()
    .with_little_endian()
    .with_variable_int_encoding()
    .write_fixed_array_length();

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_system_set(
            SystemSet::on_enter(states::GameState::InGame).with_system(create_world),
        )
        .add_system_set(
            SystemSet::on_update(states::GameState::InGame)
                .with_system(f2_prints_terrain)
                .with_system(f5_writes_terrain)
                .with_system(f6_loads_terrain)
                .with_system(g_deletes_random_block),
        )
        .add_system_set(SystemSet::on_exit(states::GameState::InGame).with_system(destroy_world));
    }
}

pub fn create_world(mut commands: Commands, assets: Res<AssetServer>) {
    info!("creating world");

    // create now, insert as resource later
    let mut terrain = Terrain::empty();

    // Generate one chunk
    create_surface_chunk(&mut commands, &assets, &mut terrain);
    spawn_chunk(1, &mut commands, assets, &mut terrain);

    let locations_to_destroy = [(5, 5), (5, 5), (300, 300), (5, 500), (2, 8)];

    // destroy some blocks
    for (x, y) in locations_to_destroy {
        info!("attempting to destroy block at ({}, {})", x, y);
        let result = destroy_block(x, y, &mut commands, &mut terrain);
        match result {
            Ok(b) => info!(
                "successfully destroyed block at ({}, {}), was type {:?}",
                x, y, b.block_type
            ),
            Err(e) => warn!("unable to destroy block at ({}, {}): {:?}", x, y, e),
        }
    }

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
#[derive(Encode, Decode, Debug, PartialEq)]
pub struct Terrain {
    /// Vector of chunks, each one contains its own chunk_number
    /// TODO: potentially convert into a symbol table for faster lookups?
    pub chunks: Vec<Chunk>,
}

impl Terrain {
    /// Create a terrain with specified number of chunks
    /// Chunks contain default blocks and are numbered from 0 to len-1
    fn new(num_chunks: u64) -> Terrain {
        Terrain {
            chunks: (0..num_chunks).map(|d| Chunk::new(d)).collect(),
        }
    }

    /// Creates a terrain with no chunks
    fn empty() -> Terrain {
        Terrain { chunks: Vec::new() }
    }
}

/// Represents a chunk of blocks; stored in the Terrain resource
/// TODO: maybe custom bitpack for Encode and Decode?
#[derive(Encode, Decode, Debug, PartialEq)]
pub struct Chunk {
    /// 2D array [x, y]
    pub blocks: [[Option<Block>; CHUNK_WIDTH]; CHUNK_HEIGHT],
    /// starting row for blocks is chunk_number * CHUNK_HEIGHT
    chunk_number: u64,
}

impl Chunk {
    pub fn new(depth: u64) -> Self {
        // For now: populate entire Chunk with Sandstone
        let c = Chunk {
            blocks: [[Some(Block {
                block_type: BlockType::Sandstone,
                entity: None,
            }); CHUNK_WIDTH]; CHUNK_HEIGHT],
            chunk_number: depth,
        };
        return c;
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
    Sandstone,
}

impl BlockType {
    /// Return the file path for the image that should be displayed for this block
    const fn image_file_path(&self) -> &str {
        match self {
            BlockType::Sandstone => "Sandstone.png",
        }
    }
}

/// Create all blocks in chunk as actual entities (and store references to entity in chunk.blocks)
pub fn spawn_chunk(
    chunk_number: u64,
    commands: &mut Commands,
    assets: Res<AssetServer>,
    terrain: &mut Terrain,
) {
    let mut chunk = Chunk::new(chunk_number);

    // Loop through entire chunk (2D Array)
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

    // add the chunk to our terrain resource
    terrain.chunks.push(chunk);
}


pub fn create_surface_chunk(
    commands: &mut Commands,
    assets: &Res<AssetServer>,
    terrain: &mut Terrain
){
    let mut chunk = Chunk::new(0);

    let random_vals = procedural_functions::generate_random_values(
        82981925815, 
        16,
        1,
        16,
        );
    // Loop through entire chunk (2D Array)
    for x in 0..CHUNK_WIDTH {
        for y in procedural_functions::slice_pos_x(x,&random_vals,10).round() as usize-1..CHUNK_HEIGHT {
            info!(y);
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
                                to_world_point_y(y, 0),
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

    // add the chunk to our terrain resource
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
                            info!("despawning sprite for block at ({}, {})", x, y);
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
                    warn!("no block exists at ({}, {})", x, y);
                    return Err(DestroyBlockError::BlockDoesntExist);
                }
            }
        }
    }

    Err(DestroyBlockError::ChunkNotLoaded)
}

pub fn to_world_point_x(x: usize) -> f32 {
    return (x as f32) * 32.;
}
pub fn to_world_point_y(y: usize, chunk_number: u64) -> f32 {
    return -(y as f32 + chunk_number as f32 * CHUNK_HEIGHT as f32) * 32.;
}

fn print_encoding_sizes() {
    match bincode::encode_to_vec(Block::new(BlockType::Sandstone), BINCODE_CONFIG) {
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

// Make f5 write encoded terrain to file "savedterrain"
fn f5_writes_terrain(input: Res<Input<KeyCode>>, terrain: Res<Terrain>) {
    // return early if F5 was not just pressed
    if !input.just_pressed(KeyCode::F5) {
        return;
    }
    // try to encode, allocating a vec
    // in a real packet, we should use a pre-allocated array and encode into its slice
    match bincode::encode_to_vec(terrain.as_ref(), BINCODE_CONFIG) {
        Ok(encoded_vec) => {
            // write the bytes to file
            create_dir_all("./savedata/"); //creates the savedata folder if it is missing

            match File::create("./savedata/quicksave.sav") {
                Ok(mut file) => {
                    file.write_all(&encoded_vec)
                        .expect("could not write to save file");
                }
                Err(e) => {
                    error!("could not create save file, {}", e);
                }
            }
        }
        Err(e) => {
            error!("unable to encode terrain, {}", e);
        }
    }
}

// Make f6 read encoded terrain from file "savedterrain", clear current block entities, and link new entities
fn f6_loads_terrain(
    input: Res<Input<KeyCode>>,
    mut commands: Commands,
    assets: Res<AssetServer>,
    query: Query<Entity, With<RenderedBlock>>,
) {
    // return early if F6 was not just pressed
    if !input.just_pressed(KeyCode::F6) {
        return;
    }
    match read("./savedata/quicksave.sav") {
        Ok(encoded_vec) => {
            // clear the world
            for entity in query.iter() {
                commands.entity(entity).despawn();
            }
            commands.remove_resource::<Terrain>();
            // load the world
            let mut decoded: Terrain = bincode::decode_from_slice(&encoded_vec, BINCODE_CONFIG)
                .unwrap()
                .0;
            load_from_vec(&mut commands, assets, &mut decoded);
            commands.insert_resource(decoded);
        }
        Err(e) => {
            error!("could not read save file, {}", e);
        }
    }
}

// Load world from vec (assumes terrain is cleared)
pub fn load_from_vec(commands: &mut Commands, assets: Res<AssetServer>, terrain: &mut Terrain) {
    for chunk in &mut terrain.chunks {
        for x in 0..CHUNK_WIDTH {
            for y in 0..CHUNK_HEIGHT {
                let block_opt = &mut chunk.blocks[x][y];
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
        rand::thread_rng().gen_range(0, CHUNK_WIDTH),
        rand::thread_rng().gen_range(0, CHUNK_HEIGHT),
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
        let original = Block::new(BlockType::Sandstone);
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
            chunk.blocks[1][1] = Some(Block::new(BlockType::Sandstone));
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
            terrain.chunks[1].blocks[1][1] = Some(Block::new(BlockType::Sandstone));
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
        let block_size = bincode::encode_to_vec(Block::new(BlockType::Sandstone), BINCODE_CONFIG)
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
