//! Video support: thumbnails and in-app playback.
//!
//! On Android this drives the platform's `MediaMetadataRetriever` and `MediaPlayer` through JNI.
//! Frames are rendered by `MediaPlayer` into a `SurfaceTexture`, which we draw into the egui
//! window with a small GL paint callback, so no Java code or extra activity is needed.

use eframe::egui::ColorImage;
use std::path::Path;

pub const VIDEO_EXT: &[&str] = &["mp4", "mkv", "webm", "3gp", "3g2", "mov", "m4v", "ts", "m2ts", "mts", "avi", "mpg", "mpeg", "wmv", "flv", "ogv"];

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| VIDEO_EXT.contains(&x.to_ascii_lowercase().as_str()))
}

pub struct VideoThumb {
    pub image: ColorImage,
    pub duration_ms: u64,
}

pub fn format_duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(target_os = "android")]
pub use android::{Player, init, thumbnail};
#[cfg(not(target_os = "android"))]
pub use stub::{Player, thumbnail};

#[cfg(not(target_os = "android"))]
mod stub {
    use super::*;
    use eframe::egui;

    pub fn thumbnail(_path: &Path, _px: u32) -> Option<VideoThumb> {
        None
    }

    pub struct Player;

    impl Player {
        pub fn open(_path: &Path) -> Option<Player> {
            None
        }
        pub fn size(&self) -> (i32, i32) {
            (16, 9)
        }
        pub fn duration_ms(&self) -> i32 {
            0
        }
        pub fn position_ms(&self) -> i32 {
            0
        }
        pub fn is_playing(&self) -> bool {
            false
        }
        pub fn has_frame(&self) -> bool {
            true
        }
        pub fn play(&self) {}
        pub fn pause(&self) {}
        pub fn seek(&self, _ms: i32) {}
        pub fn set_looping(&self, _on: bool) {}
        pub fn set_speed(&self, _speed: f32) {}
        pub fn set_muted(&self, _muted: bool) {}
        pub fn paint_callback(&self, rect: egui::Rect) -> egui::PaintCallback {
            egui::PaintCallback { rect, callback: std::sync::Arc::new(()) }
        }
    }
}

#[cfg(target_os = "android")]
mod android {
    use super::*;
    use eframe::egui;
    use eframe::glow::{self, HasContext};
    use jni::objects::{GlobalRef, JObject, JString, JValue};
    use jni::{JNIEnv, JavaVM};
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    static JVM: OnceLock<JavaVM> = OnceLock::new();
    static ACTIVITY: OnceLock<GlobalRef> = OnceLock::new();
    /// The MediaPlayer that is currently open in the gallery (serial, player), watched for Home presses.
    static CURRENT: Mutex<Option<(u64, GlobalRef)>> = Mutex::new(None);

    pub fn init(app: &android_activity::AndroidApp) {
        // SAFETY: the pointer comes from android-activity and is valid for the life of the process.
        if let Ok(vm) = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) } {
            let _ = JVM.set(vm);
        }
        // SAFETY: the activity pointer stays valid while the app runs; from_raw does not take ownership.
        let act = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
        with_env(|env| {
            let _ = ACTIVITY.set(env.new_global_ref(&act)?);
            Ok(())
        });
        // The window loses focus when Home is pressed, but the UI may then stop running, so a
        // small watcher thread pauses playback instead of relying on the UI to notice.
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let Some((_, mp)) = CURRENT.lock().unwrap().clone() else { continue };
                let Some(act) = ACTIVITY.get() else { continue };
                with_env(|env| {
                    let focused = env.call_method(act.as_obj(), "hasWindowFocus", "()Z", &[])?.z()?;
                    if !focused && env.call_method(mp.as_obj(), "isPlaying", "()Z", &[])?.z()? {
                        call_void(env, mp.as_obj(), "pause", "()V", &[])?;
                    }
                    Ok(())
                });
            }
        });
    }

    /// Run `f` with a JNI environment for this thread; any pending Java exception is cleared.
    fn with_env<R>(f: impl FnOnce(&mut JNIEnv) -> jni::errors::Result<R>) -> Option<R> {
        let mut env = JVM.get()?.attach_current_thread_permanently().ok()?;
        let r = f(&mut env);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            return None;
        }
        r.ok()
    }

    fn call_void(env: &mut JNIEnv, obj: &JObject, name: &str, sig: &str, args: &[JValue]) -> jni::errors::Result<()> {
        env.call_method(obj, name, sig, args)?;
        Ok(())
    }

    fn metadata_int(env: &mut JNIEnv, r: &JObject, key: i32) -> Option<i64> {
        let s = env
            .call_method(r, "extractMetadata", "(I)Ljava/lang/String;", &[JValue::Int(key)])
            .ok()?
            .l()
            .ok()?;
        if s.is_null() {
            return None;
        }
        let js: String = env.get_string(&JString::from(s)).ok()?.into();
        js.trim().parse().ok()
    }

    /// First-frame thumbnail plus duration, via MediaMetadataRetriever.
    pub fn thumbnail(path: &Path, px: u32) -> Option<VideoThumb> {
        with_env(|env| {
            let r = env.new_object("android/media/MediaMetadataRetriever", "()V", &[])?;
            let result = (|| -> jni::errors::Result<Option<VideoThumb>> {
                let jpath = env.new_string(path.to_string_lossy())?;
                call_void(env, &r, "setDataSource", "(Ljava/lang/String;)V", &[JValue::Object(&jpath)])?;
                let duration_ms = metadata_int(env, &r, 9).unwrap_or(0).max(0) as u64;
                let (mut w, mut h) = (
                    metadata_int(env, &r, 18).unwrap_or(16) as f32,
                    metadata_int(env, &r, 19).unwrap_or(9) as f32,
                );
                if matches!(metadata_int(env, &r, 24), Some(90 | 270)) {
                    std::mem::swap(&mut w, &mut h);
                }
                let scale = (px as f32 / w.max(h)).min(1.0);
                let (tw, th) = (((w * scale) as i32).max(2), ((h * scale) as i32).max(2));
                let at = (duration_ms as i64 * 1000 / 10).min(1_000_000); // ~10% in, at most 1 s: skips black first frames
                let bmp = env
                    .call_method(
                        &r,
                        "getScaledFrameAtTime",
                        "(JIII)Landroid/graphics/Bitmap;",
                        &[JValue::Long(at), JValue::Int(2), JValue::Int(tw), JValue::Int(th)],
                    )?
                    .l()?;
                if bmp.is_null() {
                    return Ok(None);
                }
                let bw = env.call_method(&bmp, "getWidth", "()I", &[])?.i()?;
                let bh = env.call_method(&bmp, "getHeight", "()I", &[])?.i()?;
                let arr = env.new_int_array(bw * bh)?;
                call_void(
                    env,
                    &bmp,
                    "getPixels",
                    "([IIIIIII)V",
                    &[JValue::Object(&arr), JValue::Int(0), JValue::Int(bw), JValue::Int(0), JValue::Int(0), JValue::Int(bw), JValue::Int(bh)],
                )?;
                let mut px_buf = vec![0i32; (bw * bh) as usize];
                env.get_int_array_region(&arr, 0, &mut px_buf)?;
                call_void(env, &bmp, "recycle", "()V", &[])?;
                let rgba: Vec<u8> = px_buf
                    .iter()
                    .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255])
                    .collect();
                Ok(Some(VideoThumb {
                    image: ColorImage::from_rgba_unmultiplied([bw as usize, bh as usize], &rgba),
                    duration_ms,
                }))
            })();
            let _ = call_void(env, &r, "release", "()V", &[]);
            result
        })
        .flatten()
    }

    // ---------- GL drawing of the SurfaceTexture ----------

    const GL_TEXTURE_EXTERNAL_OES: u32 = 0x8D65;
    static SERIAL: AtomicU64 = AtomicU64::new(1);

    struct GlObjs {
        tex: glow::Texture,
        program: glow::Program,
        vao: glow::VertexArray,
        u_mat: glow::UniformLocation,
        u_tex: glow::UniformLocation,
        u_rect: glow::UniformLocation,
        attached_serial: u64,
    }

    // glow handles are plain ids; they are only ever used from the render thread.
    unsafe impl Send for GlObjs {}

    static GL_OBJS: Mutex<Option<GlObjs>> = Mutex::new(None);

    // The quad is placed with uRect = (left, bottom, right, top) in clip space, computed from the
    // full, unclipped rectangle. Relying on the GL viewport instead squeezes the picture whenever
    // it is partly off screen (for example while swiping to the next video).
    const VERT: &str = "attribute vec2 aUv; uniform vec4 uRect; uniform mat4 uMat; varying vec2 vUv;\n\
        void main() {\n\
            gl_Position = vec4(mix(uRect.x, uRect.z, aUv.x), mix(uRect.y, uRect.w, aUv.y), 0.0, 1.0);\n\
            vUv = (uMat * vec4(aUv, 0.0, 1.0)).xy;\n\
        }";
    const FRAG: &str = "#extension GL_OES_EGL_image_external : require\n\
        precision mediump float; varying vec2 vUv; uniform samplerExternalOES uTex;\n\
        void main() { gl_FragColor = texture2D(uTex, vUv); }";

    unsafe fn create_gl(gl: &glow::Context) -> Option<GlObjs> {
        unsafe {
            let compile = |kind, src: &str| -> Option<glow::Shader> {
                let s = gl.create_shader(kind).ok()?;
                gl.shader_source(s, src);
                gl.compile_shader(s);
                if !gl.get_shader_compile_status(s) {
                    log::error!("video shader: {}", gl.get_shader_info_log(s));
                    return None;
                }
                Some(s)
            };
            let vs = compile(glow::VERTEX_SHADER, VERT)?;
            let fs = compile(glow::FRAGMENT_SHADER, FRAG)?;
            let program = gl.create_program().ok()?;
            gl.attach_shader(program, vs);
            gl.attach_shader(program, fs);
            gl.bind_attrib_location(program, 0, "aUv");
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                log::error!("video program: {}", gl.get_program_info_log(program));
                return None;
            }
            gl.delete_shader(vs);
            gl.delete_shader(fs);

            let vao = gl.create_vertex_array().ok()?;
            let vbo = gl.create_buffer().ok()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            // unit quad: (0,0) bottom-left .. (1,1) top-right
            let verts: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck_cast(&verts), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
            gl.bind_vertex_array(None);

            let tex = gl.create_texture().ok()?;
            gl.bind_texture(GL_TEXTURE_EXTERNAL_OES, Some(tex));
            gl.tex_parameter_i32(GL_TEXTURE_EXTERNAL_OES, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(GL_TEXTURE_EXTERNAL_OES, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(GL_TEXTURE_EXTERNAL_OES, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(GL_TEXTURE_EXTERNAL_OES, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            gl.bind_texture(GL_TEXTURE_EXTERNAL_OES, None);

            Some(GlObjs {
                tex,
                program,
                vao,
                u_mat: gl.get_uniform_location(program, "uMat")?,
                u_tex: gl.get_uniform_location(program, "uTex")?,
                u_rect: gl.get_uniform_location(program, "uRect")?,
                attached_serial: 0,
            })
        }
    }

    fn bytemuck_cast(v: &[f32]) -> &[u8] {
        // SAFETY: f32 has no padding or invalid bit patterns; the length is scaled to bytes.
        unsafe { std::slice::from_raw_parts(v.as_ptr().cast(), std::mem::size_of_val(v)) }
    }

    fn draw(gl: &glow::Context, st: &GlobalRef, serial: u64, rect: egui::Rect, info: &egui::PaintCallbackInfo, has_frame: &AtomicBool) {
        let mut guard = GL_OBJS.lock().unwrap();
        if guard.is_none() {
            // SAFETY: called from egui_glow's paint callback, where the GL context is current.
            *guard = unsafe { create_gl(gl) };
        }
        let Some(objs) = guard.as_mut() else { return };

        let matrix = with_env(|env| {
            if objs.attached_serial != serial {
                let id = objs.tex.0.get() as i32;
                call_void(env, st.as_obj(), "attachToGLContext", "(I)V", &[JValue::Int(id)])?;
                objs.attached_serial = serial;
            }
            call_void(env, st.as_obj(), "updateTexImage", "()V", &[])?;
            // 0 until the first frame of THIS video has arrived; until then the shared texture
            // still holds the previous video's last frame, which must not be drawn.
            let timestamp = env.call_method(st.as_obj(), "getTimestamp", "()J", &[])?.j()?;
            let arr = env.new_float_array(16)?;
            call_void(env, st.as_obj(), "getTransformMatrix", "([F)V", &[JValue::Object(&arr)])?;
            let mut m = [0f32; 16];
            env.get_float_array_region(&arr, 0, &mut m)?;
            Ok((m, timestamp))
        });
        let Some((m, timestamp)) = matrix else { return };
        if timestamp == 0 {
            return;
        }
        has_frame.store(true, Ordering::Relaxed);

        // rectangle in clip space, from the full (unclipped) rect; egui has already set the scissor
        let (ppp, [sw, sh]) = (info.pixels_per_point, info.screen_size_px);
        let (sw, sh) = (sw as f32, sh as f32);
        let left = rect.min.x * ppp / sw * 2.0 - 1.0;
        let right = rect.max.x * ppp / sw * 2.0 - 1.0;
        let top = 1.0 - rect.min.y * ppp / sh * 2.0;
        let bottom = 1.0 - rect.max.y * ppp / sh * 2.0;

        // SAFETY: GL context is current on this thread (see above).
        unsafe {
            gl.viewport(0, 0, sw as i32, sh as i32);
            gl.use_program(Some(objs.program));
            gl.uniform_4_f32(Some(&objs.u_rect), left, bottom, right, top);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(GL_TEXTURE_EXTERNAL_OES, Some(objs.tex));
            gl.uniform_1_i32(Some(&objs.u_tex), 0);
            gl.uniform_matrix_4_f32_slice(Some(&objs.u_mat), false, &m);
            gl.bind_vertex_array(Some(objs.vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.bind_vertex_array(None);
            gl.bind_texture(GL_TEXTURE_EXTERNAL_OES, None);
        }
    }

    // ---------- player ----------

    pub struct Player {
        mp: GlobalRef,
        st: GlobalRef,
        surface: GlobalRef,
        serial: u64,
        w: i32,
        h: i32,
        duration: i32,
        /// Set by the paint callback once this video's first frame has been drawn.
        has_frame: Arc<AtomicBool>,
        looping: AtomicBool,
        muted: AtomicBool,
        speed_bits: AtomicU32,
    }

    impl Player {
        pub fn open(path: &Path) -> Option<Player> {
            with_env(|env| {
                let st = env.new_object("android/graphics/SurfaceTexture", "(Z)V", &[JValue::Bool(0)])?;
                let surface = env.new_object("android/view/Surface", "(Landroid/graphics/SurfaceTexture;)V", &[JValue::Object(&st)])?;
                let mp = env.new_object("android/media/MediaPlayer", "()V", &[])?;
                let jpath = env.new_string(path.to_string_lossy())?;
                call_void(env, &mp, "setSurface", "(Landroid/view/Surface;)V", &[JValue::Object(&surface)])?;
                call_void(env, &mp, "setDataSource", "(Ljava/lang/String;)V", &[JValue::Object(&jpath)])?;
                call_void(env, &mp, "prepare", "()V", &[])?;
                let w = env.call_method(&mp, "getVideoWidth", "()I", &[])?.i()?;
                let h = env.call_method(&mp, "getVideoHeight", "()I", &[])?.i()?;
                let duration = env.call_method(&mp, "getDuration", "()I", &[])?.i()?;
                call_void(env, &st, "setDefaultBufferSize", "(II)V", &[JValue::Int(w.max(1)), JValue::Int(h.max(1))])?;
                call_void(env, &mp, "start", "()V", &[])?;
                let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
                let mp = env.new_global_ref(&mp)?;
                *CURRENT.lock().unwrap() = Some((serial, mp.clone()));
                Ok(Player {
                    mp,
                    st: env.new_global_ref(&st)?,
                    surface: env.new_global_ref(&surface)?,
                    serial,
                    w,
                    h,
                    duration,
                    has_frame: Arc::new(AtomicBool::new(false)),
                    looping: AtomicBool::new(false),
                    muted: AtomicBool::new(false),
                    speed_bits: AtomicU32::new(1.0f32.to_bits()),
                })
            })
        }

        pub fn size(&self) -> (i32, i32) {
            (self.w.max(1), self.h.max(1))
        }

        pub fn duration_ms(&self) -> i32 {
            self.duration
        }

        pub fn position_ms(&self) -> i32 {
            with_env(|env| env.call_method(self.mp.as_obj(), "getCurrentPosition", "()I", &[])?.i()).unwrap_or(0)
        }

        pub fn is_playing(&self) -> bool {
            with_env(|env| env.call_method(self.mp.as_obj(), "isPlaying", "()Z", &[])?.z()).unwrap_or(false)
        }

        /// True once the first picture of this video has been drawn.
        pub fn has_frame(&self) -> bool {
            self.has_frame.load(Ordering::Relaxed)
        }

        pub fn play(&self) {
            with_env(|env| call_void(env, self.mp.as_obj(), "start", "()V", &[]));
        }

        pub fn pause(&self) {
            with_env(|env| call_void(env, self.mp.as_obj(), "pause", "()V", &[]));
        }

        /// Jump to `ms`, landing exactly on that frame (not the nearest keyframe), so a paused
        /// video shows the picture for the point the finger is on.
        pub fn seek(&self, ms: i32) {
            const SEEK_CLOSEST: i32 = 3;
            with_env(|env| call_void(env, self.mp.as_obj(), "seekTo", "(JI)V", &[JValue::Long(ms as i64), JValue::Int(SEEK_CLOSEST)]));
        }

        pub fn set_muted(&self, muted: bool) {
            if self.muted.swap(muted, Ordering::Relaxed) != muted {
                let v = if muted { 0.0f32 } else { 1.0 };
                with_env(|env| call_void(env, self.mp.as_obj(), "setVolume", "(FF)V", &[JValue::Float(v), JValue::Float(v)]));
            }
        }

        /// Playback speed (0.5 to 2). Changing it must not start a paused video, which some
        /// Android versions do, so a paused player is paused again afterwards.
        pub fn set_speed(&self, speed: f32) {
            if self.speed_bits.swap(speed.to_bits(), Ordering::Relaxed) == speed.to_bits() {
                return;
            }
            let was_playing = self.is_playing();
            with_env(|env| {
                let p = env.new_object("android/media/PlaybackParams", "()V", &[])?;
                env.call_method(&p, "setSpeed", "(F)Landroid/media/PlaybackParams;", &[JValue::Float(speed)])?;
                call_void(env, self.mp.as_obj(), "setPlaybackParams", "(Landroid/media/PlaybackParams;)V", &[JValue::Object(&p)])
            });
            if !was_playing {
                self.pause();
            }
        }

        pub fn set_looping(&self, on: bool) {
            if self.looping.swap(on, Ordering::Relaxed) != on {
                with_env(|env| call_void(env, self.mp.as_obj(), "setLooping", "(Z)V", &[JValue::Bool(on as u8)]));
            }
        }

        /// A paint callback that draws the current video frame into `rect`.
        pub fn paint_callback(&self, rect: egui::Rect) -> egui::PaintCallback {
            let (st, serial, has_frame) = (self.st.clone(), self.serial, self.has_frame.clone());
            let cb = eframe::egui_glow::CallbackFn::new(move |info, painter| draw(painter.gl(), &st, serial, rect, &info, &has_frame));
            egui::PaintCallback { rect, callback: Arc::new(cb) }
        }
    }

    impl Drop for Player {
        fn drop(&mut self) {
            {
                let mut cur = CURRENT.lock().unwrap();
                if cur.as_ref().is_some_and(|(s, _)| *s == self.serial) {
                    *cur = None;
                }
            }
            with_env(|env| {
                let _ = call_void(env, self.mp.as_obj(), "stop", "()V", &[]);
                call_void(env, self.mp.as_obj(), "release", "()V", &[])?;
                call_void(env, self.surface.as_obj(), "release", "()V", &[])?;
                call_void(env, self.st.as_obj(), "release", "()V", &[])
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_extensions_and_durations() {
        assert!(is_video(Path::new("/a/b/clip.MP4")));
        assert!(is_video(Path::new("x.webm")));
        assert!(!is_video(Path::new("photo.jpg")));
        assert!(!is_video(Path::new("noext")));
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(65_000), "1:05");
        assert_eq!(format_duration(3_723_000), "1:02:03");
    }
}
