//! Video thumbnails (first frame and length) through Android's MediaMetadataRetriever.

use eframe::egui::ColorImage;
use std::path::Path;

pub struct VideoThumb {
    pub image: ColorImage,
    pub duration_ms: u64,
}

#[cfg(not(target_os = "android"))]
pub fn thumbnail(_path: &Path, _px: u32) -> Option<VideoThumb> {
    None
}

#[cfg(not(target_os = "android"))]
pub fn init_stub() {}

#[cfg(target_os = "android")]
pub use android::{init, thumbnail};

#[cfg(target_os = "android")]
mod android {
    use super::*;
    use jni::objects::{JObject, JString, JValue};
    use jni::{JNIEnv, JavaVM};
    use std::sync::OnceLock;

    static JVM: OnceLock<JavaVM> = OnceLock::new();

    pub fn init(app: &android_activity::AndroidApp) {
        // SAFETY: the pointer comes from android-activity and is valid for the life of the process.
        if let Ok(vm) = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) } {
            let _ = JVM.set(vm);
        }
    }

    /// Run `f` with a JNI environment for this thread; any pending Java exception is cleared.
    fn with_env<R>(f: impl FnOnce(&mut JNIEnv) -> jni::errors::Result<R>) -> Option<R> {
        let mut env = JVM.get()?.attach_current_thread_permanently().ok()?;
        let r = f(&mut env);
        if env.exception_check().unwrap_or(false) {
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
        let s = env.call_method(r, "extractMetadata", "(I)Ljava/lang/String;", &[JValue::Int(key)]).ok()?.l().ok()?;
        if s.is_null() {
            return None;
        }
        let js: String = env.get_string(&JString::from(s)).ok()?.into();
        js.trim().parse().ok()
    }

    /// First-frame thumbnail plus duration.
    pub fn thumbnail(path: &Path, px: u32) -> Option<VideoThumb> {
        with_env(|env| {
            let r = env.new_object("android/media/MediaMetadataRetriever", "()V", &[])?;
            let result = (|| -> jni::errors::Result<Option<VideoThumb>> {
                let jpath = env.new_string(path.to_string_lossy())?;
                call_void(env, &r, "setDataSource", "(Ljava/lang/String;)V", &[JValue::Object(&jpath)])?;
                let duration_ms = metadata_int(env, &r, 9).unwrap_or(0).max(0) as u64;
                let (mut w, mut h) = (metadata_int(env, &r, 18).unwrap_or(16) as f32, metadata_int(env, &r, 19).unwrap_or(9) as f32);
                if matches!(metadata_int(env, &r, 24), Some(90 | 270)) {
                    std::mem::swap(&mut w, &mut h);
                }
                let scale = (px as f32 / w.max(h)).min(1.0);
                let (tw, th) = (((w * scale) as i32).max(2), ((h * scale) as i32).max(2));
                let at = (duration_ms as i64 * 1000 / 10).min(1_000_000);
                let bmp = env
                    .call_method(&r, "getScaledFrameAtTime", "(JIII)Landroid/graphics/Bitmap;", &[JValue::Long(at), JValue::Int(2), JValue::Int(tw), JValue::Int(th)])?
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
                let rgba: Vec<u8> = px_buf.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
                Ok(Some(VideoThumb { image: ColorImage::from_rgba_unmultiplied([bw as usize, bh as usize], &rgba), duration_ms }))
            })();
            let _ = call_void(env, &r, "release", "()V", &[]);
            result
        })
        .flatten()
    }
}
