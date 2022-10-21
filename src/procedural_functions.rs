use bevy::prelude::*;
use rand::{rngs::StdRng, SeedableRng, Rng};

use crate::world::CHUNK_WIDTH;


//Generates vector of random values, with seed, with amount
pub fn generate_random_values(seed: u64, amount: usize, low: usize, high: usize) -> Vec<i32>{
    let mut values: Vec<i32> = Vec::new();

    let mut rand = StdRng::seed_from_u64(seed);
    for n in 0..amount{
        let value: i32 = rand.gen_range(low as i32, high as i32);
        values.push(value);
    }
    values
}

//[]32
//[1,6,3,5,7]

//Get the value (float) of a position X
pub fn slice_pos_x(x: usize, r: &Vec<i32>, divisions: i32) -> f32{
    
    // 128 -- chunk_width
    // 2  -- 
    // 64 -- r.len


    //Make x value 1/10 the value... so we can generate spaces between points
    let x_float = (x as f32) / ((CHUNK_WIDTH/r.len()) + 1) as f32; //2.0f32; //as f32 // (r.len()) as f32; //(divisions as f32);

    // Get integer version of 1/10 the value
    let x_int = x_float as u32; 

    //Get diff (distance)
    let diff = x_float - (x_int as f32); 

    //Cubic curve
    let u = diff * diff * (3.0 - 2.0 * diff); 

    //32 / 12 32/12 2 8/12

    //Interpolate + return
    //if(x_int < r.len())

    return (r[x_int as usize]) as f32 *(1.0f32-u) + ((r[(x_int+1) as usize]) as f32 * u); 
    
}