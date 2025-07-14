use crate::block::Block;
use godot::classes::*;
use godot::global::{Key, MouseButton};
use godot::prelude::*;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::ops::DerefMut;
use std::time::Duration;
use godot_tokio::AsyncRuntime;
use tokio::sync::broadcast::{Receiver, Sender, channel};
use tokio::time::sleep;

// Node structure for A* algorithm
#[derive(Copy, Clone, Eq, PartialEq)]
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

    start_block: Option<(i32, i32)>,
    end_block: Option<(i32, i32)>,
}

impl Default for AStarController {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            blocks: vec![],
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
        self.step_mode_label.set_text(self.step_mode.to_string().as_str());

        let block_prefab = load::<PackedScene>("res://Block.tscn");
        let mut container = self.base().get_node_as::<GridContainer>("%GridContainer");
        let mut rng = RandomNumberGenerator::new_gd();
        rng.randomize();
        self.seed_label.set_text(rng.get_seed().to_string().as_str());

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
                    self.step_mode_label.set_text(self.step_mode.to_string().as_str());
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
        let mut open_set = BinaryHeap::new();
        let mut closed_set = HashSet::new();

        // Initialize came_from map to reconstruct the path
        let mut came_from = HashMap::new();

        // Initialize g_scores map (cost from start to current node)
        let mut g_scores = HashMap::new();
        g_scores.insert(start_pos, 0);

        // Add start node to open set
        let h_score = Self::manhattan_distance(start_pos, end_pos);
        let f_score = 0 + h_score;
        godot_print!(
            "Initializing open set with start node at position {:?} with f_score={}, g_score=0, h_score={}",
            start_pos,
            f_score,
            h_score
        );
        open_set.push(Node::new(start_pos, 0, h_score));

        let mut last_block: Option<Gd<Block>> = None;

        // Main A* loop
        while let Some(current) = open_set.pop() {
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
                self.reconstruct_path(&came_from, end_pos);
                return;
            }

            // Skip if already in closed set
            if closed_set.contains(&current_pos) {
                godot_print!(
                    "Node at position {:?} is already in closed set, skipping",
                    current_pos
                );
                continue;
            }

            // Add to closed set and visualize
            closed_set.insert(current_pos);
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

            // Store the current f_score for finding nodes with the same f_score
            let current_f_score = current.f_score;

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
                if closed_set.contains(&neighbor_pos) {
                    godot_print!(
                        "Neighbor at position {:?} is already in closed set, skipping",
                        neighbor_pos
                    );
                    continue;
                }

                // Calculate tentative g_score
                let tentative_g_score = *g_scores.get(&current_pos).unwrap_or(&i32::MAX);
                let previous_g_score = *g_scores.get(&neighbor_pos).unwrap_or(&i32::MAX);

                godot_print!(
                    "Neighbor at position {:?}: tentative g_score={}, previous g_score={}",
                    neighbor_pos,
                    tentative_g_score,
                    previous_g_score
                );

                // If this path is better than any previous one, update
                if tentative_g_score < previous_g_score {
                    godot_print!(
                        "Found better path to neighbor at position {:?}",
                        neighbor_pos
                    );
                    // Update came_from map
                    came_from.insert(neighbor_pos, current_pos);

                    // Update g_score
                    g_scores.insert(neighbor_pos, tentative_g_score);

                    // Calculate h_score
                    let h_score = Self::manhattan_distance(neighbor_pos, end_pos);
                    let f_score = tentative_g_score + h_score;

                    godot_print!(
                        "Adding node at position {:?} to open set with f_score={}, g_score={}, h_score={}",
                        neighbor_pos,
                        f_score,
                        tentative_g_score,
                        h_score
                    );

                    // Add to open set
                    open_set.push(Node::new(neighbor_pos, tentative_g_score, h_score));

                    // Visualize open set (but don't color start and end blocks)
                    if neighbor_pos != start_pos && neighbor_pos != end_pos {
                        if let Some(mut block) = self.get_block(neighbor_pos.0, neighbor_pos.1) {
                            // Update block's f, g, h values
                            block.bind_mut().set_f(tentative_g_score + h_score);
                            block.bind_mut().set_g(tentative_g_score);
                            block.bind_mut().set_h(h_score);

                            // Only color if not already in closed set (which would be colored differently)
                            if !closed_set.contains(&neighbor_pos) {
                                block.bind_mut().set_color(Game::OPEN_BLOCK_COLOR);
                            }
                        }
                    }
                }
            }

            // Extract and process all nodes with the same f_score as the current node
            let mut same_f_score_nodes = Vec::new();

            // Use BinaryHeap::peek to look at the next node without removing it
            while let Some(next_node) = open_set.peek() {
                if next_node.f_score == current_f_score {
                    // If it has the same f_score, pop it and add to our list
                    let node = open_set.pop().unwrap();
                    same_f_score_nodes.push(node);
                } else {
                    // If it has a different f_score, stop
                    break;
                }
            }

            godot_print!(
                "Found {} additional nodes with the same f_score={}",
                same_f_score_nodes.len(),
                current_f_score
            );

            // Process all nodes with the same f_score
            for node in same_f_score_nodes {
                let node_pos = node.position;

                godot_print!(
                    "Processing additional node at position {:?} with f_score={}, g_score={}, h_score={}",
                    node_pos,
                    node.f_score,
                    node.g_score,
                    node.h_score
                );

                // Skip if already in closed set
                if closed_set.contains(&node_pos) {
                    godot_print!(
                        "Additional node at position {:?} is already in closed set, skipping",
                        node_pos
                    );
                    continue;
                }

                // If we reached the end, reconstruct and return the path
                if node_pos == end_pos {
                    godot_print!("Reached end position {:?} with additional node! Path found!", end_pos);
                    godot_print!("A* algorithm finished successfully");

                    // Update came_from to ensure we can reconstruct the path
                    came_from.insert(node_pos, current_pos);
                    g_scores.insert(node_pos, node.g_score);

                    self.reconstruct_path(&came_from, end_pos);
                    return;
                }

                // Add to closed set and visualize
                closed_set.insert(node_pos);
                godot_print!("Added additional node at position {:?} to closed set", node_pos);

                // Don't color start and end blocks
                if node_pos != start_pos && node_pos != end_pos {
                    if let Some(mut block) = self.get_block(node_pos.0, node_pos.1) {
                        // Update block's f, g, h values
                        block.bind_mut().set_f(node.f_score);
                        block.bind_mut().set_g(node.g_score);
                        block.bind_mut().set_h(node.h_score);

                        // Color as closed (processed) block
                        block.bind_mut().set_color(Game::CLOSED_BLOCK_COLOR);
                    }
                }

                // Process neighbors of this node too
                let node_neighbors = self.get_neighbors(node_pos);
                godot_print!(
                    "Found {} neighbors for additional node at position {:?}",
                    node_neighbors.len(),
                    node_pos
                );

                for neighbor_pos in node_neighbors {
                    godot_print!("Processing neighbor at position {:?} of additional node", neighbor_pos);

                    // Skip if in closed set
                    if closed_set.contains(&neighbor_pos) {
                        godot_print!(
                            "Neighbor at position {:?} is already in closed set, skipping",
                            neighbor_pos
                        );
                        continue;
                    }

                    // Calculate tentative g_score
                    let tentative_g_score = *g_scores.get(&node_pos).unwrap_or(&i32::MAX);
                    let previous_g_score = *g_scores.get(&neighbor_pos).unwrap_or(&i32::MAX);

                    godot_print!(
                        "Neighbor at position {:?}: tentative g_score={}, previous g_score={}",
                        neighbor_pos,
                        tentative_g_score,
                        previous_g_score
                    );

                    // If this path is better than any previous one, update
                    if tentative_g_score < previous_g_score {
                        godot_print!(
                            "Found better path to neighbor at position {:?}",
                            neighbor_pos
                        );
                        // Update came_from map
                        came_from.insert(neighbor_pos, node_pos);

                        // Update g_score
                        g_scores.insert(neighbor_pos, tentative_g_score);

                        // Calculate h_score
                        let h_score = Self::manhattan_distance(neighbor_pos, end_pos);
                        let f_score = tentative_g_score + h_score;

                        godot_print!(
                            "Adding node at position {:?} to open set with f_score={}, g_score={}, h_score={}",
                            neighbor_pos,
                            f_score,
                            tentative_g_score,
                            h_score
                        );

                        // Add to open set
                        open_set.push(Node::new(neighbor_pos, tentative_g_score, h_score));

                        // Visualize open set (but don't color start and end blocks)
                        if neighbor_pos != start_pos && neighbor_pos != end_pos {
                            if let Some(mut block) = self.get_block(neighbor_pos.0, neighbor_pos.1) {
                                // Update block's f, g, h values
                                block.bind_mut().set_f(tentative_g_score + h_score);
                                block.bind_mut().set_g(tentative_g_score);
                                block.bind_mut().set_h(h_score);

                                // Only color if not already in closed set (which would be colored differently)
                                if !closed_set.contains(&neighbor_pos) {
                                    block.bind_mut().set_color(Game::OPEN_BLOCK_COLOR);
                                }
                            }
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
    fn reconstruct_path(
        &mut self,
        came_from: &HashMap<(i32, i32), (i32, i32)>,
        end_pos: (i32, i32),
    ) {
        godot_print!("Reconstructing path from end position {:?}", end_pos);

        let mut current = end_pos;
        let mut path = Vec::new();

        // Reconstruct the path by following came_from map
        while let Some(&prev) = came_from.get(&current) {
            path.push(current);
            godot_print!("Path node: {:?} <- {:?}", current, prev);
            current = prev;

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
                AsyncRuntime::runtime().spawn(async {
                    sleep(Duration::from_millis(100)).await;
                }).await.unwrap();
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
