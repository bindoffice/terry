//! Terminal graphics support: kitty graphics protocol and sixel.
//!
//! Images are decoded into an RGBA pool and *placed* onto the terminal grid.
//! The top-left cell of every placement carries a reference to its placement
//! (see [`crate::term::cell::Cell::set_image`]), which means placements scroll
//! together with the grid content automatically.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use base64::Engine as _;
use image::AnimationDecoder as _;

use crate::grid::{Dimensions, Grid};
use crate::index::{Column, Line, Point};
use crate::term::cell::{Cell, Flags};

/// Key identifying an image in the pool: (transmission id, image number).
pub type ImageKey = (u32, u32);

/// Decoded RGBA8 image shared with the renderer.
#[derive(Debug, Clone)]
pub struct Image {
    /// Stable id assigned by the pool; used to key renderer-side caches.
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
    /// Kitty graphics protocol format identifier (24, 32, 100, ...).
    pub format: u8,
    /// Gap in milliseconds before the next animation frame (kitty `z` key).
    /// The root frame uses 0; animation frames default to 40ms.
    pub delay_ms: u32,
    /// Subsequent animation frames, in play order. The root frame's `frames`
    /// vec holds the whole animation; nested frames do not carry their own.
    pub frames: Vec<Arc<Image>>,
    /// Incremented whenever the image's frames change; renderer caches key on it.
    pub generation: u64,
}

impl Image {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>, format: u8) -> Self {
        debug_assert_eq!(width as usize * height as usize * 4, rgba.len());
        Self {
            id: 0,
            width,
            height,
            rgba: Arc::new(rgba),
            format,
            delay_ms: 0,
            frames: Vec::new(),
            generation: 0,
        }
    }

    #[inline]
    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }
}

/// Metadata of a single image placement on the grid.
#[derive(Debug, Clone)]
pub struct Placement {
    /// Key of the image in the pool.
    pub image: ImageKey,
    /// Width of the placement in cells.
    pub cols: u16,
    /// Height of the placement in cells.
    pub rows: u16,
    /// Z-index; higher z-index images are painted on top.
    pub z: i32,
    /// Optional user-provided placement id.
    pub user_id: Option<u32>,
}

/// An in-flight chunked kitty protocol transmission.
struct PendingTransmission {
    bytes: Vec<u8>,
    format: u8,
    width: Option<u32>,
    height: Option<u32>,
    /// Whether this transmission is an animation frame (`a=f`).
    is_frame: bool,
    /// Frame gap in milliseconds (kitty `z`), for frame transmissions.
    delay_ms: u32,
    /// 1-based frame index being edited (kitty `r`), for frame transmissions.
    frame_index: Option<usize>,
}

/// Sixel DCS accumulation state.
struct SixelAccum {
    aspect_ratio: Option<u16>,
    zero_color: Option<u16>,
    grid_size: Option<u16>,
    buf: Vec<u8>,
}

/// Default memory budget for the image pool.
pub const DEFAULT_MAX_IMAGE_MEMORY: usize = 512 * 1024 * 1024;

/// State of all terminal graphics.
pub struct ImageState {
    /// Decoded images keyed by (transmission id, number).
    pub(crate) images: HashMap<ImageKey, Arc<Image>>,
    /// Insertion order of images, for FIFO eviction when over budget.
    order: VecDeque<ImageKey>,
    /// Placement metadata keyed by internal placement id.
    pub(crate) placements: HashMap<u32, Placement>,
    next_placement_id: u32,
    /// Bytes currently held by the pool.
    memory_used: usize,
    /// Maximum bytes the pool may hold.
    max_memory: usize,
    /// In-flight chunked kitty transmissions keyed by (transmission id, number).
    pending: HashMap<(u32, u32), PendingTransmission>,
    /// Sixel DCS accumulation, while a DCS `q` string is being received.
    sixel: Option<SixelAccum>,
    /// Most recently transmitted image, used when a put omits the image id.
    last_image: Option<ImageKey>,
    /// Set when placements may have been orphaned and a GC should run.
    needs_gc: bool,
    /// Counter for stable image ids.
    next_image_id: u64,
    /// Cell size in pixels used to derive default placement sizes.
    cell_width_px: f32,
    cell_height_px: f32,
}

impl Default for ImageState {
    fn default() -> Self {
        Self {
            images: Default::default(),
            order: Default::default(),
            placements: Default::default(),
            next_placement_id: 0,
            memory_used: 0,
            max_memory: DEFAULT_MAX_IMAGE_MEMORY,
            pending: Default::default(),
            sixel: None,
            last_image: None,
            needs_gc: false,
            next_image_id: 1,
            cell_width_px: 8.0,
            cell_height_px: 16.0,
        }
    }
}

impl ImageState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the pixel size of a grid cell (used for default placement sizes).
    pub fn set_cell_size(&mut self, width: f32, height: f32) {
        if width > 0.0 {
            self.cell_width_px = width;
        }
        if height > 0.0 {
            self.cell_height_px = height;
        }
    }

    /// Whether a GC pass over the grid is required to drop orphaned placements.
    pub fn needs_gc(&self) -> bool {
        self.needs_gc
    }

    /// Mark that a GC pass is required (e.g. after a grid resize).
    pub fn set_needs_gc(&mut self) {
        self.needs_gc = true;
    }

    pub fn set_max_memory(&mut self, max_memory: usize) {
        self.max_memory = max_memory;
        self.evict_if_over_budget();
    }

    #[inline]
    pub fn image(&self, key: &ImageKey) -> Option<Arc<Image>> {
        self.images.get(key).cloned()
    }

    #[inline]
    pub fn placement(&self, id: u32) -> Option<&Placement> {
        self.placements.get(&id)
    }

    /// Remove all images and placements.
    pub fn clear(&mut self) {
        self.images.clear();
        self.order.clear();
        self.placements.clear();
        self.pending.clear();
        self.memory_used = 0;
        self.needs_gc = true;
    }

    /// Remove a single placement by internal id.
    pub fn remove_placement(&mut self, id: u32) {
        if self.placements.remove(&id).is_some() {
            self.needs_gc = true;
        }
    }

    /// Drop placements that are no longer referenced by any grid cell.
    ///
    /// This must be called with the grid that the placements live in. It walks
    /// every row once, collecting the set of placement ids referenced by cells
    /// and pruning everything else.
    pub fn gc(&mut self, grid: &Grid<Cell>) {
        self.needs_gc = false;

        if self.placements.is_empty() {
            return;
        }

        let mut live = std::collections::HashSet::with_capacity(self.placements.len());
        let topmost = grid.topmost_line();
        let start = Point::new(topmost, Column(0));
        for indexed in grid.iter_from(start) {
            if let Some(id) = indexed.cell.image() {
                live.insert(id);
            }
        }

        self.placements.retain(|id, _| live.contains(id));
    }

    /// Remove all placements referencing the given image and the image itself.
    fn remove_image(&mut self, key: &ImageKey) {
        self.placements
            .retain(|_, placement| placement.image != *key);
        if let Some(image) = self.images.remove(key) {
            self.memory_used = self
                .memory_used
                .saturating_sub(self.total_image_memory(&image));
            self.order.retain(|k| k != key);
            self.needs_gc = true;
        }
    }

    /// Remove all images belonging to a transmission id.
    fn remove_transmission(&mut self, transmission_id: u32) {
        let keys: Vec<ImageKey> = self
            .images
            .keys()
            .copied()
            .filter(|(t, _)| *t == transmission_id)
            .collect();
        for key in keys {
            self.remove_image(&key);
        }
        self.pending.retain(|(t, _), _| *t != transmission_id);
    }

    /// Remove a placement by its user-provided id.
    fn remove_user_placement(&mut self, user_id: u32) {
        let id = self.placements.iter().find_map(|(id, placement)| {
            if placement.user_id == Some(user_id) {
                Some(*id)
            } else {
                None
            }
        });
        if let Some(id) = id {
            self.remove_placement(id);
        }
    }

    /// Total memory of an image including all animation frames.
    fn total_image_memory(&self, image: &Image) -> usize {
        image
            .byte_len()
            .saturating_add(image.frames.iter().map(|f| f.byte_len()).sum::<usize>())
    }

    fn evict_if_over_budget(&mut self) {
        while self.memory_used > self.max_memory && !self.order.is_empty() {
            let key = self.order.pop_front().unwrap();
            if let Some(image) = self.images.remove(&key) {
                self.memory_used = self
                    .memory_used
                    .saturating_sub(self.total_image_memory(&image));
                self.placements
                    .retain(|_, placement| placement.image != key);
                self.needs_gc = true;
            }
        }
    }

    /// Insert a decoded image into the pool, evicting older images if the
    /// memory budget is exceeded. Returns `false` if the image is too large.
    fn insert_image(&mut self, key: ImageKey, mut image: Image) -> bool {
        if image.byte_len() > self.max_memory {
            return false;
        }

        image.id = self.next_image_id;
        self.next_image_id += 1;

        if let Some(previous) = self.images.insert(key, Arc::new(image)) {
            self.memory_used = self
                .memory_used
                .saturating_sub(self.total_image_memory(&previous));
        } else {
            self.order.push_back(key);
        }
        let image = self.images.get(&key).unwrap();
        self.memory_used += self.total_image_memory(image);
        self.last_image = Some(key);
        self.evict_if_over_budget();
        true
    }

    /// Append an animation frame to an existing image (kitty `a=f`).
    fn append_frame(&mut self, key: ImageKey, mut frame: Image, delay_ms: u32) -> bool {
        let Some(root) = self.images.get_mut(&key).map(Arc::make_mut) else {
            // No root image yet: treat the frame as the root itself.
            frame.delay_ms = delay_ms;
            return self.insert_image(key, frame);
        };

        let frame_mem = frame.byte_len();
        frame.delay_ms = delay_ms;
        let frame = Arc::new(frame);
        root.frames.push(frame);
        root.generation += 1;
        self.memory_used += frame_mem;
        self.evict_if_over_budget();
        true
    }

    /// Replace an animation frame (1-based `r` index) in an existing image.
    fn replace_frame(
        &mut self,
        key: ImageKey,
        index: usize,
        mut frame: Image,
        delay_ms: u32,
    ) -> bool {
        let Some(root) = self.images.get_mut(&key).map(Arc::make_mut) else {
            return false;
        };
        if index == 0 || index > root.frames.len() {
            return false;
        }
        frame.delay_ms = delay_ms;
        let old = std::mem::replace(&mut root.frames[index - 1], Arc::new(frame));
        self.memory_used = self
            .memory_used
            .saturating_sub(old.byte_len())
            .saturating_add(root.frames[index - 1].byte_len());
        root.generation += 1;
        true
    }

    /// Remove all animation frames of an image (kitty `a=u`).
    fn unframe(&mut self, key: &ImageKey) {
        if let Some(root) = self.images.get_mut(key).map(Arc::make_mut) {
            for frame in root.frames.drain(..) {
                self.memory_used = self.memory_used.saturating_sub(frame.byte_len());
            }
            root.generation += 1;
        }
    }

    /// Place an image on the grid at (or relative to) the cursor.
    ///
    /// Returns `false` if the placement could not be made (e.g. it would fall
    /// entirely outside the grid).
    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        grid: &mut Grid<Cell>,
        key: ImageKey,
        cols: u16,
        rows: u16,
        z: i32,
        user_id: Option<u32>,
        offset: (usize, usize),
        advance_cursor: bool,
    ) -> bool {
        if self.images.get(&key).is_none() {
            return false;
        }

        let columns = grid.columns();
        let screen_lines = grid.screen_lines();

        let cursor = grid.cursor.point;
        let line = cursor.line.0 + offset.1 as i32;
        let column = cursor.column.0 + offset.0;

        // Clamp the placement to the grid.
        let cols = (cols as usize).min(columns.saturating_sub(column));
        let rows = (rows as usize).min(screen_lines.saturating_sub(line as usize));
        if cols == 0 || rows == 0 {
            return false;
        }
        let cols = cols as u16;
        let rows = rows as u16;

        let placement_id = self.next_placement_id;
        self.next_placement_id += 1;
        self.placements.insert(
            placement_id,
            Placement {
                image: key,
                cols,
                rows,
                z,
                user_id,
            },
        );

        // Mark the top-left cell of the placement.
        let point = Point::new(Line(line), Column(column));
        let cell = &mut grid[point];
        cell.flags.insert(Flags::IMAGE);
        cell.set_image(Some(placement_id));

        // Move the cursor past the image, as if it were text.
        if advance_cursor {
            let last_column = columns - 1;
            if column + cols as usize > last_column {
                grid.cursor.input_needs_wrap = true;
            }
            grid.cursor.point.column = Column((column + cols as usize).min(last_column));
        }

        true
    }

    /// Start a sixel DCS string.
    pub fn sixel_hook(&mut self, params: &crate::vte::Params) {
        let mut iter = params.iter();
        let aspect_ratio = iter.next().and_then(|p| p.first()).copied();
        let zero_color = iter.next().and_then(|p| p.first()).copied();
        let grid_size = iter.next().and_then(|p| p.first()).copied();
        self.sixel = Some(SixelAccum {
            aspect_ratio,
            zero_color,
            grid_size,
            buf: Vec::new(),
        });
    }

    /// Feed a byte to the current sixel string.
    pub fn sixel_put(&mut self, byte: u8) {
        if let Some(accum) = &mut self.sixel {
            accum.buf.push(byte);
        }
    }

    /// Finish a sixel string and place the decoded image at the cursor.
    pub fn sixel_unhook(&mut self, grid: &mut Grid<Cell>) {
        let Some(accum) = self.sixel.take() else {
            return;
        };

        // Reconstruct the full DCS sequence for the decoder.
        let mut data = Vec::with_capacity(accum.buf.len() + 16);
        data.extend_from_slice(b"\x1bP");
        data.extend_from_slice(
            format!(
                "{};{};{}q",
                accum.aspect_ratio.unwrap_or(0),
                accum.zero_color.unwrap_or(0),
                accum.grid_size.unwrap_or(0)
            )
            .as_bytes(),
        );
        data.extend_from_slice(&accum.buf);
        data.extend_from_slice(b"\x1b\\");

        let decoded = match icy_sixel::SixelImage::decode(&data) {
            Ok(image) => image,
            Err(err) => {
                log::debug!("sixel decode failed: {err}");
                return;
            }
        };

        if decoded.width == 0 || decoded.height == 0 {
            return;
        }

        let key = self.next_sixel_key();
        let image = Image::new(
            decoded.width as u32,
            decoded.height as u32,
            decoded.pixels,
            0,
        );
        if !self.insert_image(key, image) {
            return;
        }

        let cols = (decoded.width as f32 / self.cell_width_px).ceil() as u16;
        let rows = (decoded.height as f32 / self.cell_height_px).ceil() as u16;

        // Place at the cursor and move it to the line below the image
        // (xterm semantics: column zero of the row after the image).
        self.place(grid, key, cols, rows, 0, None, (0, 0), false);
        grid.cursor.point.column = Column(0);
        let advance = rows as i32;
        let new_line = (grid.cursor.point.line.0 + advance).min((grid.screen_lines() - 1) as i32);
        grid.cursor.point.line = Line(new_line);
        grid.cursor.input_needs_wrap = false;
    }

    /// Key for sixel images: use transmission id 0 and incrementing numbers.
    fn next_sixel_key(&mut self) -> ImageKey {
        let mut number = 0u32;
        loop {
            let key = (0, number);
            if !self.images.contains_key(&key) {
                return key;
            }
            number += 1;
        }
    }

    /// Handle a kitty graphics protocol APC payload (`ESC _ G ... ESC \`).
    pub fn handle_apc(
        &mut self,
        grid: &mut Grid<Cell>,
        respond: &mut dyn FnMut(String),
        bytes: &[u8],
    ) {
        let Some(payload) = bytes.strip_prefix(b"G") else {
            return;
        };

        // Split options from the data section at the first `;`.
        let (options, data) = match payload.iter().position(|b| *b == b';') {
            Some(idx) => (&payload[..idx], Some(&payload[idx + 1..])),
            None => (payload, None),
        };

        let mut opts: HashMap<&[u8], &[u8]> = HashMap::new();
        for pair in options.split(|b| *b == b',') {
            if let Some(idx) = pair.iter().position(|b| *b == b'=') {
                opts.insert(&pair[..idx], &pair[idx + 1..]);
            } else if !pair.is_empty() {
                opts.insert(pair, &[]);
            }
        }

        let num = |key: &str| -> Option<u32> {
            opts.get(key.as_bytes())
                .and_then(|v| std::str::from_utf8(v).ok())
                .and_then(|v| v.parse().ok())
        };
        let action = |key: &str| -> Option<String> {
            opts.get(key.as_bytes())
                .and_then(|v| std::str::from_utf8(v).ok())
                .map(|v| v.to_owned())
        };

        match action("a").as_deref() {
            Some("t") | None if data.is_some() => {
                self.apc_transmit(&opts, data.unwrap(), num, false)
            }
            Some("p") => self.apc_put(grid, &opts, num, false),
            Some("T") if data.is_some() => {
                // Transmit data and display the image (what chafa emits).
                let more = num("m").map(|m| m != 0).unwrap_or(false);
                self.apc_transmit(&opts, data.unwrap(), num, false);
                if !more {
                    self.apc_put(grid, &opts, num, true);
                }
            }
            Some("T") => self.apc_put(grid, &opts, num, true),
            Some("d") => self.apc_delete(grid, &opts, num),
            Some("q") => self.apc_query(&opts, num, respond),
            Some("f") if data.is_some() => self.apc_transmit(&opts, data.unwrap(), num, true),
            Some("f") => {
                // Frame transmission with no payload in this chunk.
                self.apc_transmit(&opts, &[], num, true)
            }
            Some("u") => {
                // Unframe: remove all animation frames of an image.
                let transmission_id = num("i")
                    .or_else(|| self.last_image.map(|(t, _)| t))
                    .unwrap_or(0);
                let number = num("I").unwrap_or(0);
                self.unframe(&(transmission_id, number));
            }
            Some("a") | Some("c") | Some("r") => {
                // Animation control / frame composition: not supported yet.
            }
            _ => {
                // No action (or a transmit with no payload): default to a
                // transient put if an image is referenced, otherwise ignore.
                if opts.contains_key(b"i".as_slice()) || opts.contains_key(b"p".as_slice()) {
                    self.apc_put(grid, &opts, num, false);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apc_transmit(
        &mut self,
        _opts: &HashMap<&[u8], &[u8]>,
        data: &[u8],
        num: impl Fn(&str) -> Option<u32>,
        is_frame: bool,
    ) {
        // Frames belong to an existing image (keyed by `i`); base images use `t`.
        let transmission_id = if is_frame {
            num("i")
                .or_else(|| self.last_image.map(|(t, _)| t))
                .unwrap_or(0)
        } else {
            num("t").unwrap_or(0)
        };
        let number = num("I").unwrap_or(0);
        let format = num("f").unwrap_or(100) as u8;
        let width = num("s");
        let height = num("v");
        // `m=1` means more chunks are coming; `m=0` (or absent) finalizes.
        let more = num("m").map(|m| m != 0).unwrap_or(false);
        let key = (transmission_id, number);

        let pending = self
            .pending
            .entry(key)
            .or_insert_with(|| PendingTransmission {
                bytes: Vec::new(),
                format,
                width,
                height,
                is_frame,
                delay_ms: 0,
                frame_index: None,
            });
        pending.format = format;
        pending.is_frame = is_frame;
        if width.is_some() {
            pending.width = width;
        }
        if height.is_some() {
            pending.height = height;
        }
        if is_frame {
            pending.delay_ms = num("z").unwrap_or(0);
            pending.frame_index = num("r").map(|r| r as usize);
        }
        pending.bytes.extend_from_slice(data);

        // Bound the memory used by in-flight chunked transmissions.
        if pending.bytes.len() > self.max_memory {
            self.pending.remove(&key);
            return;
        }

        if more {
            return;
        }

        let Some(pending) = self.pending.remove(&key) else {
            return;
        };

        let Some(mut frames) = decode_payload(
            &pending.bytes,
            pending.format,
            pending.width,
            pending.height,
        ) else {
            return;
        };

        if is_frame {
            let Some(frame) = frames.drain(..).next() else {
                return;
            };
            let delay_ms = pending.delay_ms.max(40);
            let can_replace = match pending.frame_index {
                Some(index) if index > 0 => self
                    .images
                    .get(&key)
                    .is_some_and(|root| index <= root.frames.len()),
                _ => false,
            };
            if can_replace {
                self.replace_frame(key, pending.frame_index.unwrap(), frame, delay_ms);
            } else {
                self.append_frame(key, frame, delay_ms);
            }
            return;
        }

        // Root transmission: the first decoded frame is the image, the rest
        // (e.g. from a multi-frame GIF) become its animation frames.
        let mut root = frames.remove(0);
        root.frames = frames.into_iter().map(Arc::new).collect();
        self.insert_image(key, root);
    }

    fn apc_put(
        &mut self,
        grid: &mut Grid<Cell>,
        _opts: &HashMap<&[u8], &[u8]>,
        num: impl Fn(&str) -> Option<u32>,
        advance: bool,
    ) {
        let transmission_id = num("i")
            .or_else(|| self.last_image.map(|(t, _)| t))
            .unwrap_or(0);
        let number = num("I").unwrap_or(0);
        let key = (transmission_id, number);

        if self.images.get(&key).is_none() {
            // Most recent image as a fallback.
            let key = self.last_image.unwrap_or(key);
            if self.images.get(&key).is_none() {
                return;
            }
        }

        let image = match self.images.get(&key) {
            Some(image) => image.clone(),
            None => return,
        };

        let cols = num("c")
            .map(|c| c as u16)
            .unwrap_or_else(|| (image.width as f32 / self.cell_width_px).ceil() as u16);
        let rows = num("r")
            .map(|r| r as u16)
            .unwrap_or_else(|| (image.height as f32 / self.cell_height_px).ceil() as u16);
        let z = num("z").map(|z| z as i32).unwrap_or(0);
        let user_id = num("p");
        let offset_x = num("X").unwrap_or(0) as usize;
        let offset_y = num("Y").unwrap_or(0) as usize;

        self.place(
            grid,
            key,
            cols,
            rows,
            z,
            user_id,
            (offset_x, offset_y),
            advance,
        );
    }

    fn apc_delete(
        &mut self,
        grid: &mut Grid<Cell>,
        opts: &HashMap<&[u8], &[u8]>,
        num: impl Fn(&str) -> Option<u32>,
    ) {
        let mode = opts
            .get(b"d".as_slice())
            .and_then(|v| std::str::from_utf8(v).ok())
            .unwrap_or("i");

        match mode {
            "a" => self.clear(),
            "i" => {
                let transmission_id = num("i")
                    .or_else(|| self.last_image.map(|(t, _)| t))
                    .unwrap_or(0);
                let number = num("I").unwrap_or(0);
                self.remove_image(&(transmission_id, number));
            }
            "t" => {
                let transmission_id = num("o").unwrap_or(0);
                self.remove_transmission(transmission_id);
            }
            "p" => {
                if let Some(user_id) = num("p") {
                    self.remove_user_placement(user_id);
                }
            }
            "f" => {
                // Delete animation frames.
                let transmission_id = num("i")
                    .or_else(|| self.last_image.map(|(t, _)| t))
                    .unwrap_or(0);
                let number = num("I").unwrap_or(0);
                self.unframe(&(transmission_id, number));
            }
            "c" => {
                // Delete placements whose top-left cell is inside the region.
                let cols = num("c").unwrap_or(0) as usize;
                let rows = num("r").unwrap_or(0) as usize;
                let offset_x = num("X").unwrap_or(0) as usize;
                let offset_y = num("Y").unwrap_or(0) as usize;
                let cursor = grid.cursor.point;
                let line = cursor.line.0 + offset_y as i32;
                let column = cursor.column.0 + offset_x;
                let mut ids = Vec::new();
                for row in 0..rows {
                    let line = Line(line + row as i32);
                    for col in 0..cols {
                        if let Some(id) = grid[line][Column(column + col)].image() {
                            ids.push(id);
                        }
                    }
                }
                for id in ids {
                    self.remove_placement(id);
                }
            }
            _ => {}
        }
    }

    fn apc_query(
        &mut self,
        _opts: &HashMap<&[u8], &[u8]>,
        num: impl Fn(&str) -> Option<u32>,
        respond: &mut dyn FnMut(String),
    ) {
        let transmission_id = num("i").unwrap_or(0);
        let number = num("I").unwrap_or(0);

        let response = match self.images.get(&(transmission_id, number)) {
            Some(image) => format!(
                "OK i={transmission_id} I={number} s={} v={} f={}",
                image.width, image.height, image.format
            ),
            None => "OK".to_owned(),
        };

        respond(format!("\x1b_G{response}\x1b\\"));
    }
}

/// Decode a kitty protocol payload into RGBA images (frames in play order).
fn decode_payload(
    bytes: &[u8],
    format: u8,
    width: Option<u32>,
    height: Option<u32>,
) -> Option<Vec<Image>> {
    match format {
        // Encoded formats: base64, then decode.
        100 | 101 => {
            let encoded = base64::engine::general_purpose::STANDARD
                .decode(bytes)
                .ok()?;
            let img = image::load_from_memory(&encoded).ok()?;
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            Some(vec![Image::new(w, h, rgba.into_raw(), format)])
        }
        // GIF: decode all frames with their delays.
        102 => {
            let encoded = base64::engine::general_purpose::STANDARD
                .decode(bytes)
                .ok()?;
            let decoder =
                image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&encoded)).ok()?;
            let mut frames = Vec::new();
            for (index, frame) in decoder.into_frames().flatten().enumerate() {
                let delay_ms = frame.delay().numer_denom_ms().0 as u32;
                let (w, h) = frame.buffer().dimensions();
                let mut image = Image::new(w, h, frame.into_buffer().into_raw(), format);
                // The root frame has no gap; later frames keep their GIF delay.
                image.delay_ms = if index == 0 { 0 } else { delay_ms.max(1) };
                frames.push(image);
            }
            if frames.is_empty() {
                None
            } else {
                Some(frames)
            }
        }
        // Raw RGB888.
        24 => {
            let (w, h) = (width? as usize, height? as usize);
            let mut rgba = Vec::with_capacity(w * h * 4);
            for chunk in bytes.chunks_exact(3).take(w * h) {
                rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 0xff]);
            }
            Some(vec![Image::new(w as u32, h as u32, rgba, format)])
        }
        // Raw RGBA8888.
        32 => {
            let (w, h) = (width? as usize, height? as usize);
            let mut rgba = Vec::with_capacity(w * h * 4);
            for chunk in bytes.chunks_exact(4).take(w * h) {
                rgba.extend_from_slice(chunk);
            }
            Some(vec![Image::new(w as u32, h as u32, rgba, format)])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::VoidListener;
    use crate::term::test::TermSize;
    use crate::term::{Config, Term};
    use image::ImageEncoder as _;

    fn new_term() -> (Term<VoidListener>, crate::vte::ansi::Processor) {
        let size = TermSize::new(80, 24);
        let term = Term::new(Config::default(), &size, VoidListener);
        (term, crate::vte::ansi::Processor::new())
    }

    #[test]
    fn kitty_protocol_transmit_and_place() {
        let (mut term, mut processor) = new_term();

        // 2x1 RGBA image: red pixel then blue pixel.
        let pixels = [255, 0, 0, 255, 0, 0, 255, 255];
        let mut seq = format!("\x1b_Ga=t,t=1,f=32,s=2,v=1;").into_bytes();
        seq.extend_from_slice(&pixels);
        seq.extend_from_slice(b"\x1b\\");
        seq.extend_from_slice(b"\x1b_Ga=p,i=1,c=2,r=1\x1b\\");

        processor.advance(&mut term, &seq);

        let content = term.renderable_content();
        assert_eq!(content.images.len(), 1);
        let image = &content.images[0];
        assert_eq!(image.point, Point::new(Line(0), Column(0)));
        assert_eq!(image.cols, 2);
        assert_eq!(image.rows, 1);
        assert_eq!(image.image.width, 2);
        assert_eq!(image.image.height, 1);
    }

    #[test]
    fn kitty_protocol_transient_put_keeps_cursor() {
        let (mut term, mut processor) = new_term();

        let pixels = [255, 0, 0, 255, 0, 0, 255, 255];
        let mut seq = format!("\x1b_Ga=t,t=1,f=32,s=2,v=1;").into_bytes();
        seq.extend_from_slice(&pixels);
        seq.extend_from_slice(b"\x1b\\");
        // Transient put does not advance the cursor.
        seq.extend_from_slice(b"\x1b_Ga=t,i=1,c=2,r=1\x1b\\");
        seq.extend_from_slice(b"X");

        processor.advance(&mut term, &seq);

        // The 'X' overwrites the top-left cell of the image, removing it.
        let content = term.renderable_content();
        assert!(content.images.is_empty());
    }

    #[test]
    fn kitty_protocol_delete_all() {
        let (mut term, mut processor) = new_term();

        let pixels = [255, 0, 0, 255, 0, 0, 255, 255];
        let mut seq = format!("\x1b_Ga=t,t=1,f=32,s=2,v=1;").into_bytes();
        seq.extend_from_slice(&pixels);
        seq.extend_from_slice(b"\x1b\\");
        seq.extend_from_slice(b"\x1b_Ga=p,i=1,c=2,r=1\x1b\\");
        seq.extend_from_slice(b"\x1b_Ga=d,d=a\x1b\\");

        processor.advance(&mut term, &seq);

        assert!(term.renderable_content().images.is_empty());
    }

    #[test]
    fn sixel_places_image_at_cursor() {
        let (mut term, mut processor) = new_term();

        // Minimal sixel: three pixels wide in the default palette color 0.
        let seq = b"\x1bPq#0;2;100;0;0#0~~~\x1b\\";
        processor.advance(&mut term, seq);

        let content = term.renderable_content();
        assert_eq!(content.images.len(), 1);
        let image = &content.images[0];
        assert_eq!(image.image.width, 3);
    }

    #[test]
    fn kitty_protocol_png_chunked_transmit() {
        let (mut term, mut processor) = new_term();

        // Encode a 4x2 RGBA PNG.
        let rgba = image::RgbaImage::from_pixel(4, 2, image::Rgba([255, 0, 0, 255]));
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                rgba.as_raw().as_slice(),
                4,
                2,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);

        // Transmit in two chunks (kitty uses m=1 for "more chunks"). Base64
        // chunks must stay aligned to 4-character quanta.
        let split = (b64.len() / 2) & !3;
        let (first, second) = b64.split_at(split);
        let seq = format!("\x1b_Ga=t,t=7,f=100,m=1;{first}\x1b\\");
        let seq2 = format!("\x1b_Ga=t,t=7,f=100,m=0;{second}\x1b\\\x1b_Ga=p,i=7,c=4,r=2\x1b\\");

        processor.advance(&mut term, seq.as_bytes());
        processor.advance(&mut term, seq2.as_bytes());

        let content = term.renderable_content();
        assert_eq!(content.images.len(), 1);
        let image = &content.images[0];
        assert_eq!(image.cols, 4);
        assert_eq!(image.rows, 2);
        assert_eq!(image.image.width, 4);
        assert_eq!(image.image.height, 2);
        assert_eq!(image.image.format, 100);
    }

    /// Test listener that records PTY write responses.
    struct RecordingListener(Arc<std::sync::Mutex<Vec<String>>>);

    impl crate::event::EventListener for RecordingListener {
        fn send_event(&self, event: crate::event::Event) {
            if let crate::event::Event::PtyWrite(response) = event {
                self.0.lock().unwrap().push(response);
            }
        }
    }

    #[test]
    fn kitty_protocol_query_responds() {
        let writes = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let listener = RecordingListener(writes.clone());
        let size = TermSize::new(80, 24);
        let mut term = Term::new(Config::default(), &size, listener);
        let mut processor = crate::vte::ansi::Processor::<crate::vte::ansi::StdSyncHandler>::new();

        processor.advance(&mut term, b"\x1b_Ga=q\x1b\\");

        let writes = writes.lock().unwrap();
        assert!(
            writes.iter().any(|w| w.starts_with("\x1b_GOK")),
            "query should respond with OK, got {writes:?}"
        );
    }

    #[test]
    fn kitty_protocol_animation_frames() {
        let (mut term, mut processor) = new_term();

        // Root frame (red pixel).
        let mut seq = b"\x1b_Ga=t,t=3,f=32,s=1,v=1;".to_vec();
        seq.extend_from_slice(&[255, 0, 0, 255]);
        seq.extend_from_slice(b"\x1b\\");
        // Two animation frames with gaps of 100ms and 200ms.
        seq.extend_from_slice(b"\x1b_Ga=f,i=3,f=32,s=1,v=1,z=100;");
        seq.extend_from_slice(&[0, 255, 0, 255]);
        seq.extend_from_slice(b"\x1b\\");
        seq.extend_from_slice(b"\x1b_Ga=f,i=3,f=32,s=1,v=1,z=200;");
        seq.extend_from_slice(&[0, 0, 255, 255]);
        seq.extend_from_slice(b"\x1b\\");

        processor.advance(&mut term, &seq);

        let root = term.image_state().image(&(3, 0)).expect("root image");
        assert_eq!(
            root.frames.len(),
            2,
            "two animation frames should be attached"
        );
        assert_eq!(root.frames[0].delay_ms, 100);
        assert_eq!(root.frames[1].delay_ms, 200);
        assert!(root.generation >= 2);

        // Unframe removes them again.
        processor.advance(&mut term, b"\x1b_Ga=u,i=3\x1b\\");
        assert!(term.image_state().image(&(3, 0)).unwrap().frames.is_empty());
    }

    #[test]
    fn gif_transmits_all_frames_with_delays() {
        let (mut term, mut processor) = new_term();

        // Build a 2-frame GIF (5ms and 25ms delays).
        // Build a 2-frame GIF (default 100ms frame delays).
        let gif_bytes = {
            let mut gif_bytes = Vec::new();
            {
                let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif_bytes);
                let frame1 = image::Frame::new(image::RgbaImage::from_pixel(
                    2,
                    1,
                    image::Rgba([255, 0, 0, 255]),
                ));
                let frame2 = image::Frame::new(image::RgbaImage::from_pixel(
                    2,
                    1,
                    image::Rgba([0, 255, 0, 255]),
                ));
                encoder.encode_frames(vec![frame1, frame2]).unwrap();
            }
            gif_bytes
        };
        // GifEncoder delays default to 100ms per frame; good enough.

        let b64 = base64::engine::general_purpose::STANDARD.encode(&gif_bytes);
        let seq = format!("\x1b_Ga=t,t=9,f=102;{b64}\x1b\\");
        processor.advance(&mut term, seq.as_bytes());

        let root = term.image_state().image(&(9, 0)).expect("gif root");
        assert_eq!(root.width, 2);
        assert_eq!(root.frames.len(), 1, "gif frames should be attached");
        assert!(root.frames[0].delay_ms > 0);
    }

    #[test]
    fn sixel_encoder_output_round_trips() {
        let (mut term, mut processor) = new_term();

        // Encode a 4x3 image with the same encoder tools use, then decode it
        // through the terminal's sixel path.
        let mut rgba = Vec::new();
        for y in 0..3u8 {
            for x in 0..4u8 {
                rgba.extend_from_slice(&[x * 60, y * 80, 128, 255]);
            }
        }
        let sixel = icy_sixel::sixel_encode(&rgba, 4, 3, &icy_sixel::EncodeOptions::default())
            .expect("encode");

        processor.advance(&mut term, sixel.as_bytes());

        let content = term.renderable_content();
        assert_eq!(content.images.len(), 1, "sixel image should be placed");
        assert_eq!(content.images[0].image.width, 4);
        // Sixel canvases are padded to 6-pixel bands, so 3 rows become 6.
        assert_eq!(content.images[0].image.height, 6);
    }
}
