use godot::classes::*;
use godot::prelude::*;
use ffmpeg_next as ffmpeg;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use godot::classes::image::Format;

// Initialize FFmpeg
fn init_ffmpeg() -> Result<(), ffmpeg::Error> {
    ffmpeg::init()?;
    ffmpeg::log::set_level(ffmpeg::log::Level::Info);
    Ok(())
}

#[derive(GodotClass)]
#[class(init, base = Node)]
pub struct AV1VideoPlayer {
    base: Base<Node>,
    
    #[export]
    video_path: GString,
    
    #[export]
    autoplay: bool,
    
    #[export]
    loop_video: bool,
    
    #[init(val = false)]
    is_playing: bool,
    
    #[init(val = false)]
    is_initialized: bool,
    
    #[init(val = 0.0)]
    current_time: f64,
    
    #[init(val = 0.0)]
    duration: f64,
    
    texture: Option<Gd<ImageTexture>>,
    video_stream: Option<Arc<Mutex<VideoStream>>>,
    
    #[init(node = "TextureRect")]
    texture_rect: OnReady<Gd<TextureRect>>,
}

struct VideoStream {
    format_context: ffmpeg::format::context::Input,
    decoder: ffmpeg::codec::decoder::Video,
    stream_index: usize,
    frame_rate: f64,
    width: u32,
    height: u32,
    current_frame: usize,
    total_frames: usize,
}

impl VideoStream {
    fn new(path: &str) -> Result<Self, ffmpeg::Error> {
        let format_context = ffmpeg::format::input(&path)?;
        
        // Find the first video stream
        let stream = format_context.streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(ffmpeg::Error::StreamNotFound)?;
        
        let stream_index = stream.index();
        
        // Get the decoder
        let decoder_context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = decoder_context.decoder().video()?;
        
        // Get video info
        let frame_rate = f64::from(stream.rate().0) / f64::from(stream.rate().1);
        let width = decoder.width();
        let height = decoder.height();
        
        // Estimate total frames
        let duration_seconds = stream.duration() as f64 * f64::from(stream.time_base().1) / f64::from(stream.time_base().0);
        let total_frames = (duration_seconds * frame_rate) as usize;
        
        Ok(Self {
            format_context,
            decoder,
            stream_index,
            frame_rate,
            width,
            height,
            current_frame: 0,
            total_frames,
        })
    }
    
    fn decode_next_frame(&mut self) -> Result<Option<ffmpeg::frame::Video>, ffmpeg::Error> {
        let mut decoded = None;
        
        for (stream, packet) in self.format_context.packets() {
            if stream.index() == self.stream_index {
                let mut frame = ffmpeg::frame::Video::empty();
                if self.decoder.send_packet(&packet).is_ok() && self.decoder.receive_frame(&mut frame).is_ok() {
                    decoded = Some(frame);
                    self.current_frame += 1;
                    break;
                }
            }
        }
        
        // Flush the decoder
        if decoded.is_none() {
            let mut frame = ffmpeg::frame::Video::empty();
            if self.decoder.send_eof().is_ok() && self.decoder.receive_frame(&mut frame).is_ok() {
                decoded = Some(frame);
                self.current_frame += 1;
            }
        }
        
        Ok(decoded)
    }
    
    fn seek(&mut self, seconds: f64) -> Result<(), ffmpeg::Error> {
        let time_base = self.format_context.stream(self.stream_index).unwrap().time_base();
        let timestamp = (seconds / (f64::from(time_base.1) / f64::from(time_base.0))) as i64;
        
        self.format_context.seek(timestamp, 0..)?;
        self.current_frame = (seconds * self.frame_rate) as usize;
        
        // Clear decoder buffers
        self.decoder.flush();
        
        Ok(())
    }
}

#[godot_api]
impl INode for AV1VideoPlayer {
    fn process(&mut self, delta: f64) {
        if self.is_playing && self.video_stream.is_some() {
            self.current_time += delta;

            // Update the texture with the current frame
            self.update_texture();

            // Check if video has ended
            if let Some(stream) = self.video_stream.clone() {
                let stream = stream.lock().unwrap();
                if stream.current_frame >= stream.total_frames {
                    if self.loop_video {
                        self.seek(0.0);
                    } else {
                        self.stop();
                        self.signals().finished().emit();
                    }
                }
            }
        }
    }
    
    fn ready(&mut self) {
        // Initialize FFmpeg
        if let Err(e) = init_ffmpeg() {
            godot_error!("Failed to initialize FFmpeg: {}", e);
            return;
        }

        if self.autoplay {
            self.play();
        }
    }
}

#[godot_api]
impl AV1VideoPlayer {
    #[func]
    pub fn play(&mut self) {
        if !self.is_initialized {
            self.initialize();
        }
        
        self.is_playing = true;
    }
    
    #[func]
    pub fn pause(&mut self) {
        self.is_playing = false;
    }
    
    #[func]
    pub fn stop(&mut self) {
        self.is_playing = false;
        self.seek(0.0);
    }
    
    #[func]
    pub fn seek(&mut self, time_sec: f64) {
        if let Some(stream) = &self.video_stream {
            if let Err(e) = stream.lock().unwrap().seek(time_sec) {
                godot_error!("Failed to seek: {}", e);
            } else {
                self.current_time = time_sec;
                self.update_texture();
            }
        }
    }
    
    #[func]
    pub fn get_duration(&self) -> f64 {
        self.duration
    }
    
    #[func]
    pub fn get_current_time(&self) -> f64 {
        self.current_time
    }
    
    #[func]
    pub fn is_playing(&self) -> bool {
        self.is_playing
    }

    // #[func]
    // pub fn set_video_path(&mut self, path: GString) {
    //     self.video_path = path;
    //     self.is_initialized = false;
    //     self.initialize();
    // }
    
    #[signal]
    fn finished();
}

impl AV1VideoPlayer {
    fn initialize(&mut self) {
        if self.video_path.is_empty() {
            godot_error!("Video path is empty");
            return;
        }
        
        // Convert GString to String
        let path = self.video_path.to_string();
        
        // Create video stream
        match VideoStream::new(&path) {
            Ok(stream) => {
                self.duration = stream.total_frames as f64 / stream.frame_rate;
                
                // Create texture
                let image_texture = ImageTexture::new_gd();
                self.texture = Some(image_texture);
                
                // Set texture to TextureRect
                if let Some(texture) = &self.texture {
                    self.texture_rect.set_texture(&texture.clone().upcast::<Texture2D>());
                }
                
                self.video_stream = Some(Arc::new(Mutex::new(stream)));
                self.is_initialized = true;
                
                // Update texture with first frame
                self.update_texture();
            }
            Err(e) => {
                godot_error!("Failed to initialize video stream: {}", e);
            }
        }
    }
    
    fn update_texture(&mut self) {
        if let Some(stream_arc) = self.video_stream.clone() {
            let mut stream = stream_arc.lock().unwrap();
            
            match stream.decode_next_frame() {
                Ok(Some(frame)) => {
                    // Convert frame to RGB format
                    let mut rgb_frame = ffmpeg::frame::Video::empty();
                    let mut scaler = ffmpeg::software::scaling::context::Context::get(
                        frame.format(),
                        frame.width(),
                        frame.height(),
                        ffmpeg::format::Pixel::RGB24,
                        frame.width(),
                        frame.height(),
                        ffmpeg::software::scaling::flag::Flags::BILINEAR,
                    ).unwrap();
                    
                    scaler.run(&frame, &mut rgb_frame).unwrap();
                    
                    // Create Godot Image from frame data
                    let width = rgb_frame.width() as i32;
                    let height = rgb_frame.height() as i32;
                    let data = rgb_frame.data(0);
                    
                    let mut image = Image::new_gd();
                    image.set_data(
                        width,
                        height,
                        false,
                        Format::RGB8,
                        &PackedByteArray::from_iter(data.iter().copied()),
                    );
                    
                    // Update texture
                    if let Some(texture) = &self.texture {
                        texture.clone().update(&image);
                    }
                }
                Ok(None) => {
                    // No more frames
                    if self.loop_video {
                        if let Err(e) = stream.seek(0.0) {
                            godot_error!("Failed to loop video: {}", e);
                        }
                    } else {
                        self.is_playing = false;
                        self.signals().finished().emit();
                    }
                }
                Err(e) => {
                    godot_error!("Error decoding frame: {}", e);
                }
            }
        }
    }
}