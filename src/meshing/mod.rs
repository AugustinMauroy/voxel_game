use bytemuck::{Pod, Zeroable};

use crate::world::{
    blocks::Block,
    chunks::{Chunk, CHUNK_SIZE},
};

pub const WATER_HEIGHT: f32 = 0.8;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct VoxelVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
}

/// Builds a simple visible-face mesh for one chunk.
pub fn visible_faces(chunk: &Chunk) -> Vec<VoxelVertex> {
    let mut vertices = Vec::new();
    for z in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let Some(block) = chunk.get(x, y, z) else {
                    continue;
                };
                if block.is_air() {
                    continue;
                }

                for face in FACES {
                    let neighbor = [
                        x as isize + face.offset[0],
                        y as isize + face.offset[1],
                        z as isize + face.offset[2],
                    ];
                    let neighbor = if neighbor.iter().any(|&coordinate| coordinate < 0)
                        || neighbor.iter().enumerate().any(|(axis, &coordinate)| {
                            axis == 0 && coordinate >= CHUNK_SIZE as isize
                                || axis == 1 && coordinate >= CHUNK_SIZE as isize
                                || axis == 2 && coordinate >= CHUNK_SIZE as isize
                        }) {
                        None
                    } else {
                        chunk.get(
                            neighbor[0] as usize,
                            neighbor[1] as usize,
                            neighbor[2] as usize,
                        )
                    };
                    let visible = neighbor.is_none_or(|neighbor| {
                        neighbor.is_air() || (neighbor.is_water() && !block.is_water())
                    });
                    if visible {
                        let color = face_color(block, face.shade);
                        for &corner in &face.corners {
                            let mut corner = corner;
                            if block.is_water() {
                                corner[1] *= WATER_HEIGHT;
                            }
                            vertices.push(VoxelVertex {
                                position: [
                                    x as f32 + corner[0],
                                    y as f32 + corner[1],
                                    z as f32 + corner[2],
                                ],
                                color,
                            });
                        }
                    }
                }
            }
        }
    }
    vertices
}

pub fn block_vertices(position: [f32; 3], block: Block) -> Vec<VoxelVertex> {
    let mut vertices = Vec::with_capacity(FACES.len() * 6);
    for face in FACES {
        let color = face_color(block, face.shade);
        for corner in face.corners {
            vertices.push(VoxelVertex {
                position: [
                    position[0] + corner[0],
                    position[1] + corner[1],
                    position[2] + corner[2],
                ],
                color,
            });
        }
    }
    vertices
}

struct Face {
    offset: [isize; 3],
    corners: [[f32; 3]; 6],
    shade: f32,
}

const FACES: [Face; 6] = [
    Face {
        offset: [0, 1, 0],
        corners: [
            [0., 1., 0.],
            [1., 1., 1.],
            [1., 1., 0.],
            [0., 1., 0.],
            [0., 1., 1.],
            [1., 1., 1.],
        ],
        shade: 1.0,
    },
    Face {
        offset: [0, -1, 0],
        corners: [
            [0., 0., 1.],
            [1., 0., 0.],
            [1., 0., 1.],
            [0., 0., 1.],
            [0., 0., 0.],
            [1., 0., 0.],
        ],
        shade: 0.55,
    },
    Face {
        offset: [1, 0, 0],
        corners: [
            [1., 0., 0.],
            [1., 1., 1.],
            [1., 0., 1.],
            [1., 0., 0.],
            [1., 1., 0.],
            [1., 1., 1.],
        ],
        shade: 0.8,
    },
    Face {
        offset: [-1, 0, 0],
        corners: [
            [0., 0., 1.],
            [0., 1., 0.],
            [0., 0., 0.],
            [0., 0., 1.],
            [0., 1., 1.],
            [0., 1., 0.],
        ],
        shade: 0.7,
    },
    Face {
        offset: [0, 0, 1],
        corners: [
            [1., 0., 1.],
            [0., 1., 1.],
            [0., 0., 1.],
            [1., 0., 1.],
            [1., 1., 1.],
            [0., 1., 1.],
        ],
        shade: 0.75,
    },
    Face {
        offset: [0, 0, -1],
        corners: [
            [0., 0., 0.],
            [1., 1., 0.],
            [1., 0., 0.],
            [0., 0., 0.],
            [0., 1., 0.],
            [1., 1., 0.],
        ],
        shade: 0.65,
    },
];

fn face_color(block: Block, shade: f32) -> [f32; 4] {
    let base = match block.id() {
        1 => [0.28, 0.68, 0.18, 1.0],
        2 => [0.48, 0.29, 0.12, 1.0],
        4 => [0.42, 0.22, 0.08, 1.0],
        5 => [0.16, 0.52, 0.12, 1.0],
        6 => [0.76, 0.62, 0.32, 1.0],
        7 => [0.12, 0.42, 0.85, 0.72],
        _ => [0.42, 0.44, 0.46, 1.0],
    };
    [base[0] * shade, base[1] * shade, base[2] * shade, base[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_positions_are_local_to_the_chunk() {
        let mut chunk = Chunk::new([2, -1, 3]);
        chunk.set(0, 0, 0, Block::STONE);

        let vertices = visible_faces(&chunk);

        assert_eq!(vertices[0].position, [0.0, 1.0, 0.0]);
        assert!(vertices.iter().all(|vertex| vertex.position[0] <= 1.0
            && vertex.position[1] <= 1.0
            && vertex.position[2] <= 1.0));
    }

    #[test]
    fn face_triangles_wind_toward_their_outward_normals() {
        for face in FACES {
            let first = face.corners[0];
            let second = face.corners[1];
            let third = face.corners[2];
            let edge_a = [
                second[0] - first[0],
                second[1] - first[1],
                second[2] - first[2],
            ];
            let edge_b = [
                third[0] - first[0],
                third[1] - first[1],
                third[2] - first[2],
            ];
            let normal = [
                edge_a[1] * edge_b[2] - edge_a[2] * edge_b[1],
                edge_a[2] * edge_b[0] - edge_a[0] * edge_b[2],
                edge_a[0] * edge_b[1] - edge_a[1] * edge_b[0],
            ];

            assert!(
                normal
                    .iter()
                    .zip(face.offset)
                    .map(|(actual, expected)| actual * expected as f32)
                    .sum::<f32>()
                    > 0.0
            );
        }
    }

    #[test]
    fn water_surface_is_exactly_eighty_percent_high() {
        let mut chunk = Chunk::new([0, 0, 0]);
        chunk.set(1, 2, 3, Block::WATER);
        let vertices = visible_faces(&chunk);
        assert!(vertices
            .iter()
            .any(|vertex| vertex.position == [1.0, 2.8, 3.0]));
        assert!(vertices.iter().all(|vertex| vertex.position[1] >= 2.0));
    }

    #[test]
    fn adjacent_water_cells_have_no_internal_face() {
        let mut chunk = Chunk::new([0, 0, 0]);
        chunk.set(1, 2, 3, Block::WATER);
        chunk.set(2, 2, 3, Block::WATER);
        assert_eq!(visible_faces(&chunk).len(), 60);
    }
}
