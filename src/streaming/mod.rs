use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap, VecDeque},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    gameplay::EditableVoxelWorld,
    meshing::{visible_faces, VoxelVertex},
    world::terrain::TerrainGenerator,
    world::{
        blocks::Block,
        chunks::{world_to_chunk_local, Chunk},
    },
};

pub type ChunkCoord = [i64; 3];

#[derive(Clone, Copy, Debug)]
pub struct StreamingConfig {
    pub view_distance: i64,
    pub worker_count: usize,
    pub queue_capacity: usize,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            view_distance: 4,
            worker_count: 2,
            queue_capacity: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StreamingMetrics {
    pub loaded_chunks: usize,
    pub queued_jobs: usize,
    pub last_generation_ms: Option<f64>,
    pub average_generation_ms: Option<f64>,
}

#[derive(Debug)]
pub struct CompletedChunk {
    pub coordinates: ChunkCoord,
    pub chunk: Chunk,
    pub generation: u64,
    pub vertices: Vec<VoxelVertex>,
    pub generation_time: Duration,
}

#[derive(Clone, Copy, Debug)]
struct Job {
    coordinates: ChunkCoord,
    generation: u64,
    distance: i64,
}

impl Eq for Job {}

impl PartialEq for Job {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance
            && self.coordinates == other.coordinates
            && self.generation == other.generation
    }
}

impl Ord for Job {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max heap, so invert distance to make nearby jobs win.
        other
            .distance
            .cmp(&self.distance)
            .then_with(|| self.generation.cmp(&other.generation))
    }
}

impl PartialOrd for Job {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct QueueState {
    jobs: BinaryHeap<Job>,
    shutdown: bool,
}

struct JobQueue {
    state: Mutex<QueueState>,
    available: Condvar,
    capacity: usize,
}

impl JobQueue {
    fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                jobs: BinaryHeap::new(),
                shutdown: false,
            }),
            available: Condvar::new(),
            capacity: capacity.max(1),
        }
    }

    fn push(&self, job: Job) -> bool {
        let mut state = self.state.lock().expect("streaming queue lock poisoned");
        if state.shutdown || state.jobs.len() >= self.capacity {
            return false;
        }
        state.jobs.push(job);
        self.available.notify_one();
        true
    }

    fn pop(&self) -> Option<Job> {
        let mut state = self.state.lock().expect("streaming queue lock poisoned");
        loop {
            if let Some(job) = state.jobs.pop() {
                return Some(job);
            }
            if state.shutdown {
                return None;
            }
            state = self
                .available
                .wait(state)
                .expect("streaming queue lock poisoned");
        }
    }

    fn len(&self) -> usize {
        self.state
            .lock()
            .expect("streaming queue lock poisoned")
            .jobs
            .len()
    }

    fn shutdown(&self) {
        let mut state = self.state.lock().expect("streaming queue lock poisoned");
        state.shutdown = true;
        self.available.notify_all();
    }
}

struct ChunkState {
    generation: u64,
    queued: bool,
    in_flight: bool,
    ready: bool,
}

pub struct StreamingWorld {
    config: StreamingConfig,
    queue: Arc<JobQueue>,
    results: Receiver<CompletedChunk>,
    states: HashMap<ChunkCoord, ChunkState>,
    chunks: HashMap<ChunkCoord, Chunk>,
    generations: HashMap<ChunkCoord, u64>,
    generation_samples: VecDeque<f64>,
    last_generation_ms: Option<f64>,
    workers: Vec<JoinHandle<()>>,
}

impl StreamingWorld {
    pub fn new(seed: u64, config: StreamingConfig) -> Self {
        let queue = Arc::new(JobQueue::new(config.queue_capacity));
        let (result_sender, results) = mpsc::channel();
        let shutdown = queue.clone();
        let mut workers = Vec::new();
        for index in 0..config.worker_count.max(1) {
            let queue = queue.clone();
            let result_sender = result_sender.clone();
            let generator = TerrainGenerator::new(seed);
            workers.push(
                thread::Builder::new()
                    .name(format!("chunk-worker-{index}"))
                    .spawn(move || worker_loop(queue, result_sender, generator))
                    .expect("failed to spawn chunk worker"),
            );
        }
        drop(result_sender);

        Self {
            config,
            queue: shutdown,
            results,
            states: HashMap::new(),
            chunks: HashMap::new(),
            generations: HashMap::new(),
            generation_samples: VecDeque::new(),
            last_generation_ms: None,
            workers,
        }
    }

    /// Updates the desired square around the player's chunk and returns chunks
    /// that the renderer must remove from the GPU.
    pub fn update(&mut self, player_chunk: ChunkCoord) -> Vec<ChunkCoord> {
        let mut desired = Vec::new();
        for z in -self.config.view_distance..=self.config.view_distance {
            for x in -self.config.view_distance..=self.config.view_distance {
                let coordinates = [player_chunk[0] + x, 0, player_chunk[2] + z];
                desired.push((x * x + z * z, coordinates));
            }
        }
        desired.sort_unstable_by_key(|(distance, coordinates)| (*distance, *coordinates));

        let mut unloaded = Vec::new();
        let no_longer_desired: Vec<_> = self
            .states
            .keys()
            .copied()
            .filter(|coordinates| !desired.iter().any(|(_, wanted)| wanted == coordinates))
            .collect();
        for coordinates in no_longer_desired {
            self.states.remove(&coordinates);
            self.chunks.remove(&coordinates);
            unloaded.push(coordinates);
            self.bump_generation(coordinates);
        }

        for (distance, coordinates) in desired {
            if !self.states.contains_key(&coordinates) {
                let generation = self.next_generation(coordinates);
                self.states.insert(
                    coordinates,
                    ChunkState {
                        generation,
                        queued: false,
                        in_flight: false,
                        ready: false,
                    },
                );
            }
            let state = self
                .states
                .get(&coordinates)
                .expect("desired state missing");
            if state.queued || state.in_flight || state.ready {
                continue;
            }
            let generation = state.generation;
            if self.queue.push(Job {
                coordinates,
                generation,
                distance,
            }) {
                self.states
                    .get_mut(&coordinates)
                    .expect("inserted state missing")
                    .queued = true;
            }
        }
        unloaded
    }

    pub fn drain_completed(&mut self) -> Vec<CompletedChunk> {
        let mut completed = Vec::new();
        while let Ok(result) = self.results.try_recv() {
            let accepted = self
                .states
                .get(&result.coordinates)
                .is_some_and(|state| state.generation == result.generation);
            if !accepted {
                continue;
            }
            if let Some(state) = self.states.get_mut(&result.coordinates) {
                state.in_flight = false;
                state.queued = false;
                state.ready = true;
            }
            self.chunks.insert(result.coordinates, result.chunk.clone());
            let milliseconds = result.generation_time.as_secs_f64() * 1_000.0;
            self.last_generation_ms = Some(milliseconds);
            self.generation_samples.push_back(milliseconds);
            if self.generation_samples.len() > 120 {
                self.generation_samples.pop_front();
            }
            completed.push(result);
        }
        completed
    }

    pub fn metrics(&self) -> StreamingMetrics {
        let average_generation_ms = (!self.generation_samples.is_empty()).then(|| {
            self.generation_samples.iter().sum::<f64>() / self.generation_samples.len() as f64
        });
        StreamingMetrics {
            loaded_chunks: self.states.len(),
            queued_jobs: self.queue.len(),
            last_generation_ms: self.last_generation_ms,
            average_generation_ms,
        }
    }

    pub fn block_at(&self, world: [i64; 3]) -> Option<Block> {
        let (coordinates, local) = world_to_chunk_local(world);
        self.chunks
            .get(&coordinates)
            .and_then(|chunk| chunk.get(local[0], local[1], local[2]))
    }

    pub fn set_block_at(&mut self, world: [i64; 3], block: Block) -> bool {
        let (coordinates, local) = world_to_chunk_local(world);
        let Some(chunk) = self.chunks.get_mut(&coordinates) else {
            return false;
        };
        chunk.set(local[0], local[1], local[2], block)
    }

    pub fn chunk_vertices(&self, coordinates: ChunkCoord) -> Option<Vec<VoxelVertex>> {
        let chunk = self.chunks.get(&coordinates)?;
        let mut vertices = visible_faces(chunk);
        let origin = [
            coordinates[0] * crate::world::chunks::CHUNK_SIZE as i64,
            coordinates[1] * crate::world::chunks::CHUNK_SIZE as i64,
            coordinates[2] * crate::world::chunks::CHUNK_SIZE as i64,
        ];
        for vertex in &mut vertices {
            vertex.position[0] += origin[0] as f32;
            vertex.position[1] += origin[1] as f32;
            vertex.position[2] += origin[2] as f32;
        }
        Some(vertices)
    }

    pub fn chunk(&self, coordinates: ChunkCoord) -> Option<Chunk> {
        self.chunks.get(&coordinates).cloned()
    }

    pub fn restore_chunk(&mut self, chunk: Chunk) {
        let coordinates = chunk.coordinates();
        let generation = self.next_generation(coordinates);
        self.chunks.insert(coordinates, chunk);
        self.states.insert(
            coordinates,
            ChunkState {
                generation,
                queued: false,
                in_flight: false,
                ready: true,
            },
        );
    }

    pub fn loaded_coordinates(&self) -> impl Iterator<Item = ChunkCoord> + '_ {
        self.chunks.keys().copied()
    }

    pub fn set_view_distance(&mut self, view_distance: i64) {
        self.config.view_distance = view_distance.clamp(1, 16);
    }

    pub fn mesh_chunks_affected_by(&self, world: [i64; 3]) -> Vec<ChunkCoord> {
        let (coordinates, local) = world_to_chunk_local(world);
        let mut affected = vec![coordinates];
        for (axis, at_border) in local.iter().enumerate() {
            if *at_border == 0 {
                let mut neighbor = coordinates;
                neighbor[axis] -= 1;
                affected.push(neighbor);
            } else if *at_border + 1 == crate::world::chunks::CHUNK_SIZE {
                let mut neighbor = coordinates;
                neighbor[axis] += 1;
                affected.push(neighbor);
            }
        }
        affected
            .into_iter()
            .filter(|coordinates| self.chunks.contains_key(coordinates))
            .collect()
    }

    fn next_generation(&mut self, coordinates: ChunkCoord) -> u64 {
        let generation = self.generations.entry(coordinates).or_insert(0);
        *generation += 1;
        *generation
    }

    fn bump_generation(&mut self, coordinates: ChunkCoord) {
        self.next_generation(coordinates);
    }
}

impl Drop for StreamingWorld {
    fn drop(&mut self) {
        self.queue.shutdown();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn worker_loop(queue: Arc<JobQueue>, results: Sender<CompletedChunk>, generator: TerrainGenerator) {
    while let Some(job) = queue.pop() {
        let started = Instant::now();
        let chunk = generator.generate_chunk(job.coordinates);
        let mut vertices = visible_faces(&chunk);
        let origin = [
            job.coordinates[0] * crate::world::chunks::CHUNK_SIZE as i64,
            job.coordinates[1] * crate::world::chunks::CHUNK_SIZE as i64,
            job.coordinates[2] * crate::world::chunks::CHUNK_SIZE as i64,
        ];
        for vertex in &mut vertices {
            vertex.position[0] += origin[0] as f32;
            vertex.position[1] += origin[1] as f32;
            vertex.position[2] += origin[2] as f32;
        }

        if results
            .send(CompletedChunk {
                coordinates: job.coordinates,
                chunk,
                generation: job.generation,
                vertices,
                generation_time: started.elapsed(),
            })
            .is_err()
        {
            break;
        }
    }
}

impl crate::gameplay::VoxelWorld for StreamingWorld {
    fn block_at(&self, world: [i64; 3]) -> Option<Block> {
        StreamingWorld::block_at(self, world)
    }
}

impl EditableVoxelWorld for StreamingWorld {
    fn set_block_at(&mut self, world: [i64; 3], block: Block) -> bool {
        StreamingWorld::set_block_at(self, world, block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunks::world_to_chunk_local;

    #[test]
    fn update_keeps_a_bounded_horizontal_set() {
        let mut streaming = StreamingWorld::new(
            1,
            StreamingConfig {
                view_distance: 1,
                worker_count: 1,
                queue_capacity: 2,
            },
        );
        streaming.update([12, 0, -7]);

        assert_eq!(streaming.metrics().loaded_chunks, 9);
    }

    #[test]
    fn priority_queue_orders_nearby_jobs_first() {
        let queue = JobQueue::new(4);
        queue.push(Job {
            coordinates: [3, 0, 0],
            generation: 1,
            distance: 9,
        });
        queue.push(Job {
            coordinates: [1, 0, 0],
            generation: 1,
            distance: 1,
        });

        assert_eq!(queue.pop().expect("near job").coordinates, [1, 0, 0]);
        queue.shutdown();
    }

    #[test]
    fn world_coordinates_round_trip_at_negative_chunk_boundary() {
        let (chunk, local) = world_to_chunk_local([-33, 0, -1]);
        assert_eq!(chunk, [-2, 0, -1]);
        assert_eq!(local, [31, 0, 31]);
    }
}
