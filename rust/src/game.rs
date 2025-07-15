use crate::block::Block;
use godot::classes::*;
use godot::global::{Key, MouseButton};
use godot::prelude::*;
use godot_tokio::AsyncRuntime;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::ops::DerefMut;
use std::time::Duration;
use tokio::sync::broadcast::{Receiver, Sender, channel};
use tokio::time::sleep;

// Node structure for A* algorithm
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
struct Node {
    position: (i32, i32),
    f_score: i32, // f = g + h
    g_score: i32, // cost from start to current node
    h_score: i32, // heuristic (estimated cost from current to goal)
}

impl Node {
    fn new(position: (i32, i32), g_score: i32, h_score: i32) -> Self {
        Self {
            position,
            f_score: g_score + h_score,
            g_score,
            h_score,
        }
    }
}

// Custom ordering for the priority queue (min-heap based on f_score)
impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap (lowest f_score has highest priority)
        other
            .f_score
            .cmp(&self.f_score)
            .then_with(|| other.h_score.cmp(&self.h_score)) // Tie-breaker: prefer lower h_score
            .then_with(|| other.position.cmp(&self.position)) // Final tie-breaker: position
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone)]
struct AStarController {
    width: i32,
    height: i32,
    blocks: Vec<Vec<Gd<Block>>>,
    open_set: BinaryHeap<Node>,
    closed_set: HashSet<(i32, i32)>,
    came_from: HashMap<(i32, i32), Node>,

    start_block: Option<(i32, i32)>,
    end_block: Option<(i32, i32)>,
}

impl Default for AStarController {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            blocks: vec![],
            open_set: Default::default(),
            closed_set: Default::default(),
            came_from: Default::default(),
            start_block: None,
            end_block: None,
        }
    }
}

#[derive(GodotClass)]
#[class(init, base = CanvasLayer)]
pub struct Game {
    base: Base<CanvasLayer>,

    #[export]
    width: i32,
    #[export]
    height: i32,
    #[export]
    step_mode: bool,

    #[init(node = "%StepMode")]
    step_mode_label: OnReady<Gd<Label>>,
    #[init(node = "%Seed")]
    seed_label: OnReady<Gd<Label>>,

    controller: AStarController,
    tx: Option<Sender<bool>>,
    is_processing: bool,
}

#[godot_api]
impl ICanvasLayer for Game {
    fn ready(&mut self) {
        self.controller.width = self.width;
        self.controller.height = self.height;
        self.step_mode_label
            .set_text(self.step_mode.to_string().as_str());

        let block_prefab = load::<PackedScene>("res://Block.tscn");
        let mut container = self.base().get_node_as::<GridContainer>("%GridContainer");
        let mut rng = RandomNumberGenerator::new_gd();
        rng.set_seed(6466529302137445490);
        self.seed_label
            .set_text(rng.get_seed().to_string().as_str());

        container.set_columns(self.width);
        self.controller.blocks = vec![vec![]; self.width as usize];
        for y in 0..self.height {
            for x in 0..self.width {
                let mut block = block_prefab.instantiate_as::<Block>();
                container.add_child(&block);

                // Set position
                block.bind_mut().set_pos(x, y);

                // Randomly generate walls (20% chance)
                if rng.randf() < 0.2 {
                    block.bind_mut().set_as_wall();
                }

                self.controller.blocks.deref_mut()[x as usize].push(block);
            }
        }

        // Connect signals after all blocks are created
        for y in 0..self.height {
            for x in 0..self.width {
                let block = self.controller.blocks[x as usize][y as usize].clone();
                block
                    .signals()
                    .clicked()
                    .connect_other(self, Self::on_block_clicked);
            }
        }

        // Set up input processing for right-click events
        self.base_mut().set_process_input(true);
    }

    fn input(&mut self, event: Gd<InputEvent>) {
        let mouse_event = event.clone().try_cast::<InputEventMouseButton>();
        if let Ok(mouse_event) = mouse_event {
            if mouse_event.is_pressed() && mouse_event.get_button_index() == MouseButton::RIGHT {
                // Right click - clear start/end blocks
                self.on_block_right_clicked(); // Position doesn't matter for right-click
            }
        }

        if !self.is_processing {
            let key_event = event.try_cast::<InputEventKey>();
            if let Ok(key_event) = key_event {
                if key_event.is_pressed() && key_event.get_keycode() == Key::T {
                    self.step_mode ^= true;
                    self.step_mode_label
                        .set_text(self.step_mode.to_string().as_str());
                    godot_print!("Toggle step mode: {}", self.step_mode);
                }
            }
        } else {
            // Handle keyboard input for step mode
            if self.step_mode {
                let key_event = event.try_cast::<InputEventKey>();
                if let Ok(key_event) = key_event {
                    if key_event.is_pressed() && key_event.get_keycode() == Key::SPACE {
                        if let Some(tx) = &self.tx {
                            tx.send(true).unwrap();
                        }
                    }
                }
            }
        }
    }
}
impl Game {
    pub const START_BLOCK_COLOR: Color = Color::DARK_BLUE;
    pub const END_BLOCK_COLOR: Color = Color::BLUE;
    pub const WALL_BLOCK_COLOR: Color = Color::ORANGE_RED;
    pub const PATH_BLOCK_COLOR: Color = Color::VIOLET;
    pub const OPEN_BLOCK_COLOR: Color = Color::YELLOW;
    pub const CLOSED_BLOCK_COLOR: Color = Color::DARK_ORANGE;
    pub const CURRENT_BLOCK_COLOR: Color = Color::DARK_GREEN;
}

impl AStarController {
    pub const DIRECTIONS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)]; // Up, Right, Down, Left

    // Helper method to get a block at a specific position
    fn get_block(&self, x: i32, y: i32) -> Option<Gd<Block>> {
        if x >= 0 && x < self.width && y >= 0 && y < self.height {
            Some(self.blocks[x as usize][y as usize].clone())
        } else {
            None
        }
    }

    // Helper method to set a block as the start block
    fn set_as_start_block(&mut self, x: i32, y: i32) {
        if let Some(mut block) = self.get_block(x, y) {
            block.bind_mut().set_color(Game::START_BLOCK_COLOR);
        }
        self.start_block = Some((x, y));
    }

    // Helper method to set a block as the end block
    fn set_as_end_block(&mut self, x: i32, y: i32) {
        if let Some(mut block) = self.get_block(x, y) {
            block.bind_mut().set_color(Game::END_BLOCK_COLOR);
        }
        self.end_block = Some((x, y));
    }

    // Helper method to reset a block's color
    fn reset_block_color(&mut self, x: i32, y: i32) {
        if let Some(mut block) = self.get_block(x, y) {
            block.bind_mut().reset_color();
        }
    }

    // Calculate Manhattan distance heuristic
    fn manhattan_distance(a: (i32, i32), b: (i32, i32)) -> i32 {
        (a.0 - b.0).abs() + (a.1 - b.1).abs()
    }

    // Get neighboring positions (4-way: up, right, down, left)
    fn get_neighbors(&self, (x, y): (i32, i32)) -> Vec<(i32, i32)> {
        Self::DIRECTIONS
            .iter()
            .map(|(dx, dy)| (x + dx, y + dy))
            .filter(|&(nx, ny)| {
                // Check if the neighbor is within bounds and not a wall
                if nx >= 0 && nx < self.width && ny >= 0 && ny < self.height {
                    if let Some(block) = self.get_block(nx, ny) {
                        !block.bind().is_wall()
                    } else {
                        false
                    }
                } else {
                    false
                }
            })
            .collect()
    }

    // Calculate the path using A* algorithm
    async fn calculate_path(&mut self, mut rx: Option<Receiver<bool>>) {
        godot_print!("Starting A* algorithm");

        // Reset all non-wall blocks to their original color
        self.reset_all_non_wall_blocks();

        // Get start and end positions
        let start_pos = match self.start_block {
            Some(pos) => pos,
            None => return, // No start block set
        };

        let end_pos = match self.end_block {
            Some(pos) => pos,
            None => return, // No end block set
        };

        godot_print!("Calculating path from {:?} to {:?}", start_pos, end_pos);

        // Initialize open and closed sets
        self.open_set = BinaryHeap::new();
        self.closed_set = HashSet::new();

        // Initialize came_from map to reconstruct the path
        self.came_from = HashMap::new();

        // Add start node to open set
        let h_score = Self::manhattan_distance(start_pos, end_pos);
        let f_score = 0 + h_score;
        godot_print!(
            "Initializing open set with start node at position {:?} with f_score={}, g_score=0, h_score={}",
            start_pos,
            f_score,
            h_score
        );
        self.open_set.push(Node::new(start_pos, 0, h_score));

        let mut last_block: Option<Gd<Block>> = None;

        // Main A* loop
        while let Some(current) = self.open_set.pop() {
            if let Some(ref mut rx) = rx {
                rx.recv().await.unwrap();
            }
            let current_pos = current.position;

            godot_print!(
                "Processing node at position {:?} with f_score={}, g_score={}, h_score={} ==============================================================",
                current_pos,
                current.f_score,
                current.g_score,
                current.h_score
            );

            // If we reached the end, reconstruct and return the path
            if current_pos == end_pos {
                godot_print!("Reached end position {:?}! Path found!", end_pos);
                godot_print!("A* algorithm finished successfully");
                self.reconstruct_path();
                return;
            }

            // Skip if already in closed set
            if self.closed_set.contains(&current_pos) {
                godot_print!(
                    "Node at position {:?} is already in closed set, skipping",
                    current_pos
                );
                continue;
            }

            // Add to closed set and visualize
            self.closed_set.insert(current_pos);
            godot_print!("Added node at position {:?} to closed set", current_pos);

            // Don't color start and end blocks
            if current_pos != start_pos && current_pos != end_pos {
                let cur_block = self.get_block(current_pos.0, current_pos.1);
                if let Some(mut block) = cur_block.clone() {
                    // Update block's f, g, h values
                    block.bind_mut().set_f(current.f_score);
                    block.bind_mut().set_g(current.g_score);
                    block.bind_mut().set_h(current.h_score);

                    // Color as closed (processed) block
                    if let Some(mut block) = last_block {
                        block.bind_mut().set_color(Game::CLOSED_BLOCK_COLOR);
                    }
                    block.bind_mut().set_color(Game::CURRENT_BLOCK_COLOR);
                }

                last_block = cur_block;
            }

            // Check all neighbors
            let neighbors = self.get_neighbors(current_pos);
            godot_print!(
                "Found {} neighbors for node at position {:?}",
                neighbors.len(),
                current_pos
            );

            for neighbor_pos in neighbors {
                godot_print!("Processing neighbor at position {:?}", neighbor_pos);

                // Skip if in closed set
                if self.closed_set.contains(&neighbor_pos) {
                    godot_print!(
                        "Neighbor at position {:?} is already in closed set, skipping",
                        neighbor_pos
                    );
                    continue;
                }

                // Calculate h_score
                let h_score = Self::manhattan_distance(neighbor_pos, end_pos);
                let g_score = current.g_score + 1;
                let f_score = h_score + g_score;

                godot_print!(
                    "Adding node at position {:?} to open set with f_score={}, g_score={}, h_score={}",
                    neighbor_pos,
                    f_score,
                    g_score,
                    h_score
                );

                // Update came_from map
                // if let Some(old) = self.came_from.get(&neighbor_pos) {
                //     if current.g_score < old.g_score {
                //         self.came_from.insert(neighbor_pos, current);
                //     }
                // } else {
                //     self.came_from.insert(neighbor_pos, current);
                // }
                self.came_from
                    .entry(neighbor_pos)
                    .and_modify(|x| {
                        if current.g_score < x.g_score {
                            *x = current;
                        }
                    })
                    .or_insert(current);
                godot_print!(
                    "Node ({}, {}) <- {:?}",
                    neighbor_pos.0,
                    neighbor_pos.1,
                    current_pos
                );
                // Add to open set
                self.open_set
                    .push(Node::new(neighbor_pos, g_score, h_score));

                // Visualize open set (but don't color start and end blocks)
                if neighbor_pos != start_pos && neighbor_pos != end_pos {
                    if let Some(mut block) = self.get_block(neighbor_pos.0, neighbor_pos.1) {
                        // Update block's f, g, h values
                        block.bind_mut().set_f(f_score);
                        block.bind_mut().set_g(g_score);
                        block.bind_mut().set_h(h_score);

                        // Only color if not already in closed set (which would be colored differently)
                        if !self.closed_set.contains(&neighbor_pos) {
                            block.bind_mut().set_color(Game::OPEN_BLOCK_COLOR);
                        }
                    }
                }
            }
        }

        godot_print!("Open set is empty, no path found!");
        godot_print!(
            "A* algorithm finished without finding a path from {:?} to {:?}",
            start_pos,
            end_pos
        );
    }

    // Reconstruct the path from came_from map
    fn reconstruct_path(&mut self) {
        let end_pos = self.end_block.unwrap();
        godot_print!("Reconstructing path from end position {:?}", end_pos);

        let mut current = end_pos;
        let mut path = Vec::new();

        // Reconstruct the path by following came_from map
        while let Some(&prev) = self.came_from.get(&current) {
            path.push(current);
            godot_print!("Path node: {:?} <- {:?}", current, prev);
            current = prev.position;

            // Stop if we reached the start
            if current == self.start_block.unwrap() {
                godot_print!("Reached start position {:?}", current);
                break;
            }
        }

        // Visualize the path
        for &pos in &path {
            // Don't color start and end blocks
            if pos != self.start_block.unwrap() && pos != self.end_block.unwrap() {
                if let Some(mut block) = self.get_block(pos.0, pos.1) {
                    block.bind_mut().set_color(Game::PATH_BLOCK_COLOR);
                }
            }
        }

        godot_print!("Path found with {} steps", path.len());
    }

    // Reset all non-wall blocks to their original color
    fn reset_all_non_wall_blocks(&mut self) {
        for x in 0..self.width {
            for y in 0..self.height {
                let is_start = self.start_block == Some((x, y));
                let is_end = self.end_block == Some((x, y));
                let is_wall = if let Some(block) = self.get_block(x, y) {
                    block.bind().is_wall()
                } else {
                    false
                };

                if !is_start && !is_end && !is_wall {
                    self.reset_block_color(x, y);
                }
            }
        }
    }
}

impl Game {
    fn on_block_clicked(&mut self, x: i32, y: i32) {
        // Check if the block is a wall
        let is_wall = if let Some(block) = self.controller.get_block(x, y) {
            block.bind().is_wall()
        } else {
            return;
        };

        if is_wall {
            return; // Can't set a wall as start/end block
        }

        // Check if we need to set start or end block
        if self.controller.start_block.is_none() {
            // Set as start block
            self.controller.set_as_start_block(x, y);
        } else if self.controller.end_block.is_none() {
            // Set as end block
            self.controller.set_as_end_block(x, y);

            // Calculate path when both start and end blocks are set
            self.is_processing = true;
            let mut ctr = self.controller.clone();
            let rx = if self.step_mode {
                let (tx, rx) = channel::<bool>(1);
                self.tx = Some(tx);
                Some(rx)
            } else {
                None
            };
            let mut game = self.to_gd();
            godot::task::spawn(async move {
                ctr.calculate_path(rx).await;
                AsyncRuntime::runtime()
                    .spawn(async {
                        sleep(Duration::from_millis(100)).await;
                    })
                    .await
                    .unwrap();
                game.bind_mut().is_processing = false;
                game.bind_mut().tx = None;
            });
        }
    }

    fn on_block_right_clicked(&mut self) {
        // Clear start and end blocks and reset colors
        if let Some((x, y)) = self.controller.start_block {
            self.controller.reset_block_color(x, y);
            self.controller.start_block = None;
        }

        if let Some((x, y)) = self.controller.end_block {
            self.controller.reset_block_color(x, y);
            self.controller.end_block = None;
        }

        // Reset all path blocks
        for x in 0..self.width {
            for y in 0..self.height {
                let is_start = self.controller.start_block == Some((x, y));
                let is_end = self.controller.end_block == Some((x, y));
                let is_wall = if let Some(block) = self.controller.get_block(x, y) {
                    block.bind().is_wall()
                } else {
                    false
                };

                if !is_start && !is_end && !is_wall {
                    self.controller.reset_block_color(x, y);
                }
            }
        }
    }
}
