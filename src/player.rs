use bevy::{
    prelude::*,
    sprite::collide_aabb::{collide, Collision},
    time::Stopwatch,
};
use iyes_loopless::prelude::*;
use std::{cmp, time::Duration};

use crate::network::BINCODE_CONFIG;
use bincode::{Decode, Encode};
use std::fs::*;
use std::io::Write;

use crate::{
    states::client::GameState,
    world::{
        block_exists, derender_chunk, destroy_block, render_chunk, spawn_chunk, to_world_point_x,
        to_world_point_y, Terrain, CHUNK_HEIGHT, CHUNK_WIDTH,
    },
    CharacterCamera, WIN_H, WIN_W,
};

const PLAYER_ASSET: &str = "Ferris.png";
const PLAYER_SIZE: f32 = 32.;
const PLAYER_START_COORDS: (u64, u64) = (0, 0);
const PLAYER_SPEED: f32 = 500.;
const PLAYER_JUMP_DURATION: f32 = 0.3; //seconds
const PLAYER_MINE_DURATION: f32 = 2.; //seconds
const PLAYER_MINE_RADIUS: f32 = 3.; //number of blocks
const GRAVITY: f32 = -350.0;
const CAMERA_BOUNDS_SIZE: [f32; 2] = [1000., 500.];
const PLAYER_Z: f32 = 2.0;

#[derive(Component)]
pub struct Player;

impl Encode for Player {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        bincode::Encode::encode(&self, encoder);
        Ok(())
    }
}

impl Decode for Player {
    fn decode<D: bincode::de::Decoder>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        Ok(Self {})
    }
}

/// Contains all inputs that the client needs to tell the server
/// TODO: refactor to enum?
#[derive(Encode, Decode, Clone, Debug, Default)]
pub struct PlayerInput {
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub mine: bool, //true means the block at block_x, block_y was clicked on.
    pub block_x: usize,
    pub block_y: usize,
}

#[derive(Component)]
struct JumpDuration {
    timer: Timer,
}

#[derive(Component)]
struct MineDuration {
    timer: Stopwatch,
}

#[derive(Eq, PartialEq)]
enum PlayerJumpState {
    Jumping,
    Falling,
    NonJumping,
}

impl Default for PlayerJumpState {
    fn default() -> Self {
        PlayerJumpState::NonJumping
    }
}

#[derive(Component)]
struct JumpState {
    state: PlayerJumpState,
}

#[derive(Component, Debug)]
struct PlayerCollision {
    any: bool,
    top: Option<f32>,
    right: Option<f32>,
    bottom: Option<f32>,
    left: Option<f32>,
    inside: bool,
}

impl Default for PlayerCollision {
    fn default() -> PlayerCollision {
        PlayerCollision {
            any: false,
            top: None,
            right: None,
            bottom: None,
            left: None,
            inside: false,
        }
    }
}

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_system_set(
            ConditionSet::new()
                .run_in_state(GameState::InGame)
                .with_system(handle_camera_movement)
                .with_system(handle_movement)
                .with_system(handle_mining)
                .with_system(handle_terrain)
                .into(),
        )
        .add_enter_system(GameState::InGame, setup)
        .add_exit_system(GameState::InGame, destroy_player);
    }
}

#[derive(Component)]
pub struct CameraBoundsBox {
    pub center_coord: Vec3,
}

/// Helper method, used during loading file and during systemset enter
fn spawn_player(
    mut commands: &mut Commands,
    assets: &AssetServer,
    position: (u64, u64),
    mut camera_transform: &mut Transform,
) {
    // convert from game coordinate to bevy coordinate
    let real_x = position.0 as f32 * 32.;
    let real_y = -(position.1 as f32 * 32.);

    let camera_bounds = CameraBoundsBox {
        center_coord: Vec3::new(real_x, real_y, PLAYER_Z),
    };

    // center the camera on the player (or where the player will be)
    reset_camera(&camera_bounds, camera_transform);

    info!(
        "spawning player at position=({}, {}), real = ({}, {})",
        position.0, position.1, real_x, real_y
    );
    //Player Entity
    commands
        .spawn_bundle(SpriteBundle {
            transform: Transform {
                // render in front of blocks
                translation: Vec3::new(real_x, real_y, PLAYER_Z),
                ..default()
            },
            texture: assets.load(PLAYER_ASSET),
            sprite: Sprite {
                custom_size: Some(Vec2::splat(PLAYER_SIZE)),
                ..default()
            },
            ..default()
        })
        .insert(Player)
        .insert(JumpDuration {
            timer: Timer::new(Duration::from_secs_f32(PLAYER_JUMP_DURATION), false),
        })
        .insert(MineDuration {
            timer: Stopwatch::new(),
        })
        .insert(JumpState {
            state: PlayerJumpState::default(),
        })
        .insert(camera_bounds);
}

fn destroy_player(
    query: Query<Entity, With<Player>>,
    mut camera_query: Query<(&mut Transform, With<CharacterCamera>, Without<Player>)>,
    mut commands: Commands,
) {
    for entity in query.iter() {
        commands.entity(entity).despawn();
    }

    for mut camera in camera_query.iter_mut() {
        camera.0.translation.x = PLAYER_START_COORDS.0 as f32;
        camera.0.translation.y = PLAYER_START_COORDS.1 as f32;
    }

    commands.remove_resource::<PlayerCollision>();
    commands.remove_resource::<CharacterCamera>();
    commands.remove_resource::<CameraBoundsBox>();
    commands.remove_resource::<JumpState>();
    commands.remove_resource::<MineDuration>();
    commands.remove_resource::<JumpDuration>();
    commands.remove_resource::<Camera2dBundle>();
    commands.remove_resource::<Transform>();
}

//Handles player movement, gravity, jumpstate
fn handle_movement(
    input: Res<Input<KeyCode>>,
    mut query: Query<(
        &mut Transform,
        &mut JumpDuration,
        &mut JumpState,
        With<Player>,
    )>,
    time: Res<Time>,
    terrain: Option<Res<Terrain>>,
) {
    for (mut player_transform, mut player_jump_timer, mut player_jump_state, _player) in
        query.iter_mut()
    {
        let mut x_vel = 0.;
        let mut y_vel = 0.;

        let prev_x = player_transform.translation.x;
        let prev_y = player_transform.translation.y;

        //Player moves left
        if input.pressed(KeyCode::A) {
            x_vel -= PLAYER_SPEED * time.delta_seconds();
        }

        //Player moves right
        if input.pressed(KeyCode::D) {
            x_vel += PLAYER_SPEED * time.delta_seconds();
        }

        //When space pressed, set player to jumping and start timer
        if input.just_pressed(KeyCode::Space)
            && player_jump_state.state == PlayerJumpState::NonJumping
        {
            player_jump_timer.timer.reset();
            player_jump_state.state = PlayerJumpState::Jumping;
        }

        //Player jumps (increases in height) for PLAYER_JUMP_DURATION seconds
        if !player_jump_timer.timer.finished()
            && player_jump_state.state == PlayerJumpState::Jumping
        {
            y_vel += (PLAYER_SPEED - GRAVITY) * time.delta_seconds();
            player_jump_timer.timer.tick(time.delta());
        }

        //sets jump state as player falling
        if player_jump_timer.timer.just_finished() {
            player_jump_state.state = PlayerJumpState::Falling;
        }

        player_transform.translation.x += x_vel;
        player_transform.translation.y += y_vel;

        // prevent going past horizontal world boundaries
        player_transform.translation.x = f32::min(
            f32::max(player_transform.translation.x, 0.0),
            ((CHUNK_WIDTH - 1) * 32) as f32,
        );

        if let Some(ref terrain) = terrain {
            let mut player_collision =
                get_collisions(&player_transform, terrain, input.pressed(KeyCode::F7));

            while player_collision.any {
                // info!("There's a collision: {:?}", player_collision);
                // Check for "inside" conditions that can occur and just reset in those scenarios
                if (player_collision.left.is_some() && player_collision.right.is_some())
                    || (player_collision.top.is_some() && player_collision.bottom.is_some())
                    || player_collision.inside
                {
                    player_transform.translation.x = prev_x;
                    player_transform.translation.y = prev_y;
                    // info!("Inside collision");
                    player_collision =
                        get_collisions(&player_transform, terrain, input.pressed(KeyCode::F7));

                    if !player_collision.any {
                        break;
                    }
                }

                if player_collision.left.is_some() {
                    player_transform.translation.x = player_collision.left.unwrap();
                    // info!("Left collision");
                    player_collision =
                        get_collisions(&player_transform, terrain, input.pressed(KeyCode::F7));
                } else if player_collision.right.is_some() {
                    player_transform.translation.x = player_collision.right.unwrap();
                    // info!("Right collision");
                    player_collision =
                        get_collisions(&player_transform, terrain, input.pressed(KeyCode::F7));
                }

                if !player_collision.any {
                    break;
                }

                if player_collision.top.is_some() {
                    player_transform.translation.y = player_collision.top.unwrap();
                    // info!("Top collision");
                    player_collision =
                        get_collisions(&player_transform, terrain, input.pressed(KeyCode::F7));
                } else if player_collision.bottom.is_some() {
                    player_transform.translation.y = player_collision.bottom.unwrap();
                    // info!("Bottom collision");
                    player_collision =
                        get_collisions(&player_transform, terrain, input.pressed(KeyCode::F7));
                }
            }
        }

        //Handles Gravity
        player_transform.translation.y += GRAVITY * time.delta_seconds();

        if let Some(ref terrain) = terrain {
            let player_collision =
                get_collisions(&player_transform, terrain, input.pressed(KeyCode::F7));

            if player_collision.bottom.is_some() {
                player_transform.translation.y = player_collision.bottom.unwrap();
                player_jump_state.state = PlayerJumpState::NonJumping;
            }
        }
    }
}

fn get_collisions(
    player_transform: &Mut<Transform>,
    terrain: &Res<Terrain>,
    debug: bool,
) -> PlayerCollision {
    // Get block indices we need to check
    // Assume player is 1x1 for now

    let x_block_index = (player_transform.translation.x / 32.) as usize;
    let y_block_index = -(player_transform.translation.y / 32.) as usize;

    let sizes = Vec2 { x: 32.0, y: 32.0 };

    let mut collisions = PlayerCollision::default();

    for x_index in (cmp::max(1, x_block_index) - 1)..=(cmp::min(x_block_index + 1, CHUNK_WIDTH - 1))
    {
        for y_index in (cmp::max(1, y_block_index) - 1)..=y_block_index + 1
        //(cmp::min(y_block_index + 1, CHUNK_HEIGHT - 1))
        {
            let chunk_number = y_index / CHUNK_HEIGHT;
            let chunk_y_index = y_index - (chunk_number * CHUNK_HEIGHT);

            let block = terrain.chunks[chunk_number].blocks[chunk_y_index][x_index];
            if block.is_some() && block.unwrap().entity.is_some() {
                let block_pos = Vec3 {
                    x: to_world_point_x(x_index),
                    y: to_world_point_y(chunk_y_index, chunk_number as u64),
                    z: 2.,
                };
                let collision = collide(player_transform.translation, sizes, block_pos, sizes);
                if collision.is_some() {
                    collisions.any = true;
                    match collision {
                        Some(Collision::Top) => collisions.bottom = Some(block_pos.y + sizes.y),
                        Some(Collision::Left) => collisions.right = Some(block_pos.x - sizes.x),
                        Some(Collision::Bottom) => collisions.top = Some(block_pos.y - sizes.y),
                        Some(Collision::Right) => collisions.left = Some(block_pos.x + sizes.x),
                        Some(Collision::Inside) => collisions.inside = true,
                        None => (),
                    }
                }
                if debug {
                    info!("Block x: {}, y: {}, chunk: {}, collision: {:?}, playerxy: {:?}, blockxy: {},{}", x_index, chunk_y_index, chunk_number, collision, player_transform.translation, block_pos.x, block_pos.y);
                }
            }
        }
    }

    return collisions;
}

fn handle_camera_movement(
    mut query: Query<(&Transform, &mut CameraBoundsBox, With<Player>)>,
    mut camera_query: Query<(&mut Transform, With<CharacterCamera>, Without<Player>)>,
    input: Res<Input<KeyCode>>,
) {
    for (player_transform, mut camera_box, _player) in query.iter_mut() {
        //Likely has to be changed when multiplayer is added
        let mut camera = camera_query.single_mut();

        //Calculate distance from center based on box size
        let horizontal_dist = CAMERA_BOUNDS_SIZE[0] / 2.;
        let vert_dist = CAMERA_BOUNDS_SIZE[1] / 2.;

        //Calculates coordinates of bounds based on distance from center of camera box
        let cam_x = camera_box.center_coord[0];
        let cam_y = camera_box.center_coord[1];

        let right_bound = cam_x + horizontal_dist;
        let left_bound = cam_x - horizontal_dist;
        let top_bound = cam_y + vert_dist;
        let bottom_bound = cam_y - vert_dist;

        //Checks if player is hitting boundaries of camera box
        if player_transform.translation.x >= right_bound {
            //moves center of camera box by how far player is past boundary
            camera_box.center_coord[0] += player_transform.translation.x - right_bound;
            //moves camera accordingly
            camera.0.translation.x += player_transform.translation.x - right_bound;
        }

        if player_transform.translation.x <= left_bound {
            camera_box.center_coord[0] += player_transform.translation.x - left_bound;
            camera.0.translation.x += player_transform.translation.x - left_bound;
        }

        if player_transform.translation.y >= top_bound {
            camera_box.center_coord[1] += player_transform.translation.y - top_bound;
            camera.0.translation.y += player_transform.translation.y - top_bound;
        }

        if player_transform.translation.y <= bottom_bound {
            camera_box.center_coord[1] += player_transform.translation.y - bottom_bound;
            camera.0.translation.y += player_transform.translation.y - bottom_bound;
        }

        //DEBUGGING: Free Roam Camera with Arrow Keys
        if input.pressed(KeyCode::Right) {
            camera.0.translation.x += 25.;
        }
        if input.pressed(KeyCode::Left) {
            camera.0.translation.x -= 25.;
        }
        if input.pressed(KeyCode::Up) {
            camera.0.translation.y += 25.;
        }
        if input.pressed(KeyCode::Down) {
            camera.0.translation.y -= 25.;
        }

        //Pressing R returns camera to player after free roam
        if input.pressed(KeyCode::R) {
            camera.0.translation.x = camera_box.center_coord[0];
            camera.0.translation.y = camera_box.center_coord[1];
        }
    }
}

fn handle_mining(
    mut windows: ResMut<Windows>,
    mouse: Res<Input<MouseButton>>,
    mut query: Query<(
        &mut Transform,
        &mut CameraBoundsBox,
        &mut MineDuration,
        With<Player>,
    )>,
    mut commands: Commands,
    mut terrain: ResMut<Terrain>,
    time: Res<Time>,
) {
    let window = windows.get_primary_mut();

    if !window.is_none() {
        let win = window.unwrap();

        for (transform, camera_box, mut mine_timer, _player) in query.iter_mut() {
            let ms = win.cursor_position();

            if !ms.is_none() {
                let mouse_pos = ms.unwrap();

                //calculate distance of click from camera center
                let dist_x = mouse_pos.x - (WIN_W / 2.);
                let dist_y = mouse_pos.y - (WIN_H / 2.);

                //calculate bevy choords of click
                let game_x = camera_box.center_coord.x + dist_x;
                let game_y = camera_box.center_coord.y + dist_y;

                //calculate block coords from bevy coords
                let block_x = (game_x / 32.).round() as usize;
                let block_y = (game_y / -32.).round() as usize;

                //calculate player distance from mined blocks
                let player_x_coord = transform.translation.x;
                let player_y_coord = transform.translation.y;

                let player_x = (player_x_coord / 32.).round();
                let player_y = (player_y_coord / -32.).round();

                let mine_dist = ((block_x as f32 - player_x).powi(2)
                    + (block_y as f32 - player_y).powi(2) as f32)
                    .sqrt();

                if mouse.pressed(MouseButton::Left)
                    && mine_dist <= PLAYER_MINE_RADIUS
                    && block_exists(block_x, block_y, &mut terrain)
                {
                    if mine_timer.timer.elapsed_secs() >= PLAYER_MINE_DURATION {
                        let _res = destroy_block(block_x, block_y, &mut commands, &mut terrain);
                        mine_timer.timer.reset();
                    }

                    mine_timer.timer.tick(time.delta());
                } else if mouse.just_released(MouseButton::Left) {
                    mine_timer.timer.reset();
                }

                //DEBUGGING: Right click to instantly mine
                if mouse.pressed(MouseButton::Right) {
                    let _res = destroy_block(block_x, block_y, &mut commands, &mut terrain);
                }
            }
        }
    }
}

fn handle_terrain(
    mut query: Query<(&mut Transform, With<Player>)>,
    mut terrain: ResMut<Terrain>,
    assets: Res<AssetServer>,
    mut commands: Commands,
) {
    for (player_transform, _player) in query.iter_mut() {
        //player_transform.translation.y / CHUNK_HEIGHT
        //300/32 > 16
        let chunk_size = terrain.chunks.len();
        let chunk_number = -player_transform.translation.y as usize / CHUNK_HEIGHT / 32;
        let mid_point = ((chunk_number + 1) * CHUNK_HEIGHT * 32) - CHUNK_HEIGHT * 16;

        //If you are beneath the midpoint then you either spawn or render chunk
        if -player_transform.translation.y >= mid_point as f32 {
            if chunk_size - 1 == chunk_number {
                spawn_chunk(chunk_size as u64, &mut commands, &assets, &mut terrain);
            } else if let Some(chunk) = terrain.chunks.get_mut(chunk_number + 1) {
                if !chunk.rendered {
                    render_chunk(chunk.chunk_number, &mut commands, &assets, chunk)
                }
            };
            if chunk_number > 0 {
                if let Some(chunk) = terrain.chunks.get_mut(chunk_number - 1) {
                    if chunk.rendered {
                        derender_chunk(&mut commands, chunk);
                    }
                };
            }
        } else {
            if let Some(chunk) = terrain
                .chunks
                .get_mut(cmp::max(0, (chunk_number as i32) - 1) as usize)
            {
                if !chunk.rendered {
                    render_chunk(chunk.chunk_number, &mut commands, &assets, chunk)
                }
            };
            if let Some(chunk) = terrain.chunks.get_mut(chunk_number + 1) {
                if chunk.rendered {
                    derender_chunk(&mut commands, chunk);
                }
            };
        }
    }
}

// spawns the player at a specific position
pub fn spawn_player_pos(
    position: (u64, u64),
    mut commands: &mut Commands,
    assets: &AssetServer,
    camera_transform: &mut Transform,
) {
    spawn_player(&mut commands, assets, position, camera_transform);
}

// startup system, spawns the player at 0,0
fn setup(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut query: Query<(&mut Transform, With<CharacterCamera>, Without<Player>)>,
) {
    spawn_player(
        &mut commands,
        assets.as_ref(),
        PLAYER_START_COORDS,
        &mut query.get_single_mut().unwrap().0,
    );
}

/// Helper function, centers the camera in the camera bounds
fn reset_camera(mut camera_bounds: &CameraBoundsBox, mut camera_transform: &mut Transform) {
    camera_transform.translation.x = camera_bounds.center_coord[0];
    camera_transform.translation.y = camera_bounds.center_coord[1];
}
