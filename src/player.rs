use bevy::{
    prelude::*,
    sprite::collide_aabb::{collide, Collision},
};
use std::{cmp, time::Duration};

use crate::{
    states::GameState,
    world::{to_world_point_x, to_world_point_y, Terrain, CHUNK_HEIGHT, CHUNK_WIDTH},
    CharacterCamera,
};

const PLAYER_ASSET: &str = "Ferris.png";
const PLAYER_SIZE: f32 = 32.;
const PLAYER_START_COORDS: [f32; 3] = [0., 0., 2.];
const PLAYER_SPEED: f32 = 500.;
const PLAYER_JUMP_DURATION: f32 = 0.3; //seconds
const GRAVITY: f32 = -300.0;
const CAMERA_BOUNDS_SIZE: [f32; 2] = [1000., 500.];

#[derive(Component)]
struct Player;

#[derive(Component)]
struct JumpDuration {
    timer: Timer,
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

#[derive(Component)]
struct PlayerCollision {
    top: Option<f32>,
    right: Option<f32>,
    bottom: Option<f32>,
    left: Option<f32>,
}

impl Default for PlayerCollision {
    fn default() -> PlayerCollision {
        PlayerCollision {
            top: None,
            right: None,
            bottom: None,
            left: None,
        }
    }
}

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        //todo add destroy function
        app.add_system_set(
            SystemSet::on_update(GameState::InGame)
                .with_system(handle_camera_movement)
                .with_system(handle_movement),
        )
        .add_system_set(SystemSet::on_enter(GameState::InGame).with_system(setup));
    }
}

#[derive(Component)]
struct CameraBoundsBox {
    center_coord: Vec3,
}

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    //Player Entity
    commands
        .spawn_bundle(SpriteBundle {
            transform: Transform {
                // render in front of blocks
                translation: Vec3::from_array(PLAYER_START_COORDS),
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
        .insert(JumpState {
            state: PlayerJumpState::default(),
        })
        .insert(CameraBoundsBox {
            center_coord: Vec3::from_array(PLAYER_START_COORDS),
        });
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
            let player_collision =
                get_collisions(&player_transform, terrain, input.pressed(KeyCode::F7));

            if player_collision.left.is_some() {
                player_transform.translation.x = player_collision.left.unwrap();
            }
            if player_collision.right.is_some() {
                player_transform.translation.x = player_collision.right.unwrap();
            }
            if player_collision.top.is_some() {
                player_transform.translation.y = player_collision.top.unwrap();
            }
            if player_collision.bottom.is_some() {
                player_transform.translation.y = player_collision.bottom.unwrap();
            }
            
        }

        //lse{
            //Handles Gravity
            player_transform.translation.y += GRAVITY * time.delta_seconds();
        //}
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
        for y_index in
            (cmp::max(1, y_block_index) - 1)..=y_block_index+1//(cmp::min(y_block_index + 1, CHUNK_HEIGHT - 1))
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
                match collision {
                    Some(Collision::Top) => collisions.bottom = Some(block_pos.y + sizes.y),
                    Some(Collision::Left) => collisions.right = Some(block_pos.x - sizes.x),
                    Some(Collision::Bottom) => collisions.top = Some(block_pos.y - sizes.y),
                    Some(Collision::Right) => collisions.left = Some(block_pos.x + sizes.x),
                    _ => (),
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
    }
}
