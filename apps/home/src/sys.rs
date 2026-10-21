//! What the launcher asks Android for: the list of apps, their icons, and starting one. On a PC (the
//! desktop preview) there is a made-up list instead.

use eframe::egui::ColorImage;

#[derive(Clone, Debug, PartialEq)]
pub struct App {
    pub pkg: String,
    pub cls: String,
    pub label: String,
}

impl App {
    pub fn key(&self) -> String {
        format!("{}/{}", self.pkg, self.cls)
    }
}

/// "package<TAB>activity<TAB>label" lines, as the Java helper returns them.
pub fn parse_line(line: &str) -> Option<App> {
    let mut it = line.splitn(3, '\t');
    let (pkg, cls, label) = (it.next()?, it.next()?, it.next()?);
    (!pkg.is_empty() && !cls.is_empty()).then(|| App { pkg: pkg.to_string(), cls: cls.to_string(), label: label.trim().to_string() })
}

pub fn sort_apps(apps: &mut [App]) {
    apps.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()).then_with(|| a.pkg.cmp(&b.pkg)));
}

/// ARGB pixels (as Android gives them) to an egui image.
pub fn image_from_argb(px: &[i32], size: usize) -> ColorImage {
    let rgba: Vec<u8> = px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8]).collect();
    ColorImage::from_rgba_unmultiplied([size, size], &rgba)
}

#[cfg(target_os = "android")]
pub use android::*;

#[cfg(not(target_os = "android"))]
pub use desktop::*;

#[cfg(not(target_os = "android"))]
mod desktop {
    use super::*;

    #[derive(Clone)]
    pub struct Handle;

    pub fn list_apps(_h: &Handle) -> Vec<App> {
        let names = [
            "Calculator", "Calendar", "Camera", "Chrome", "Clock", "Contacts", "Files", "Gallery", "Maps", "Messages", "Music", "Notes", "Phone", "Photos",
            "Settings", "Spotify", "Task Manager", "WhatsApp", "YouTube", "Weather", "Wallet", "Recorder", "Compass", "Translate",
        ];
        names.iter().map(|n| App { pkg: format!("demo.{}", n.to_lowercase().replace(' ', "")), cls: "Main".into(), label: (*n).to_string() }).collect()
    }

    pub fn icon(_h: &Handle, app: &App, px: usize) -> Option<ColorImage> {
        // a coloured rounded square with the first letter's tint, so the grid looks like a grid
        let seed = app.label.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        let (r, g, b) = (80 + (seed % 150) as u8, 80 + ((seed >> 8) % 150) as u8, 80 + ((seed >> 16) % 150) as u8);
        let mut pixels = Vec::with_capacity(px * px);
        for y in 0..px {
            for x in 0..px {
                let (dx, dy) = ((x as f32 - px as f32 / 2.0).abs(), (y as f32 - px as f32 / 2.0).abs());
                let inside = dx.max(dy) < px as f32 / 2.0 - 2.0 && (dx * dx + dy * dy).sqrt() < px as f32 * 0.62;
                pixels.push(if inside { eframe::egui::Color32::from_rgb(r, g, b) } else { eframe::egui::Color32::TRANSPARENT });
            }
        }
        Some(ColorImage::new([px, px], pixels))
    }

    pub fn launch(_h: &Handle, app: &App) -> bool {
        println!("would launch {}", app.label);
        true
    }

    pub fn app_info(_h: &Handle, app: &App) {
        println!("would open the settings page of {}", app.label);
    }
}

#[cfg(target_os = "android")]
mod android {
    use super::*;
    use android_activity::AndroidApp;
    use jni::objects::{JClass, JObject, JObjectArray, JString, JValue};
    use jni::{JNIEnv, JavaVM};

    pub type Handle = AndroidApp;
    const HELPER: &str = "dev.heartnetapps.home.AppHelper";

    fn with_env<R>(app: &AndroidApp, f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>) -> Option<R> {
        // SAFETY: both pointers come from android-activity and stay valid while the app runs;
        // JObject::from_raw does not take ownership of the activity reference.
        let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
        let mut env = vm.attach_current_thread().ok()?;
        let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
        let r = f(&mut env, &activity);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
            return None;
        }
        r.ok()
    }

    fn class<'a>(env: &mut JNIEnv<'a>, act: &JObject, name: &str) -> jni::errors::Result<JClass<'a>> {
        let loader = env.call_method(act, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
        let n = env.new_string(name)?;
        let c = env.call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&n)])?.l()?;
        Ok(JClass::from(c))
    }

    pub fn list_apps(h: &Handle) -> Vec<App> {
        with_env(h, |env, act| {
            let c = class(env, act, HELPER)?;
            let arr = env.call_static_method(&c, "listApps", "(Landroid/app/Activity;)[Ljava/lang/String;", &[JValue::Object(act)])?.l()?;
            let arr = JObjectArray::from(arr);
            let n = env.get_array_length(&arr)?;
            let mut out = Vec::new();
            for i in 0..n {
                let s = JString::from(env.get_object_array_element(&arr, i)?);
                let line: String = env.get_string(&s)?.into();
                if let Some(app) = parse_line(&line) {
                    out.push(app);
                }
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    pub fn icon(h: &Handle, app: &App, px: usize) -> Option<ColorImage> {
        with_env(h, |env, act| {
            let c = class(env, act, HELPER)?;
            let (p, k) = (env.new_string(&app.pkg)?, env.new_string(&app.cls)?);
            let arr = env
                .call_static_method(&c, "icon", "(Landroid/app/Activity;Ljava/lang/String;Ljava/lang/String;I)[I", &[JValue::Object(act), JValue::Object(&p), JValue::Object(&k), JValue::Int(px as i32)])?
                .l()?;
            let arr = jni::objects::JIntArray::from(arr);
            let mut buf = vec![0i32; px * px];
            env.get_int_array_region(&arr, 0, &mut buf)?;
            Ok(image_from_argb(&buf, px))
        })
    }

    pub fn launch(h: &Handle, app: &App) -> bool {
        with_env(h, |env, act| {
            let c = class(env, act, HELPER)?;
            let (p, k) = (env.new_string(&app.pkg)?, env.new_string(&app.cls)?);
            env.call_static_method(&c, "launch", "(Landroid/app/Activity;Ljava/lang/String;Ljava/lang/String;)Z", &[JValue::Object(act), JValue::Object(&p), JValue::Object(&k)])?.z()
        })
        .unwrap_or(false)
    }

    pub fn app_info(h: &Handle, app: &App) {
        with_env(h, |env, act| {
            let c = class(env, act, HELPER)?;
            let p = env.new_string(&app.pkg)?;
            env.call_static_method(&c, "appInfo", "(Landroid/app/Activity;Ljava/lang/String;)V", &[JValue::Object(act), JValue::Object(&p)])?;
            Ok(())
        });
    }

    /// Register the Rust function behind `HomeActivity.nativeDestroying()`.
    pub fn register_destroy_hook(h: &Handle, hook: extern "system" fn(JNIEnv, JClass)) -> bool {
        with_env(h, |env, act| {
            let c = class(env, act, "dev.heartnetapps.home.HomeActivity")?;
            env.register_native_methods(&c, &[jni::NativeMethod { name: "nativeDestroying".into(), sig: "()V".into(), fn_ptr: hook as *mut std::ffi::c_void }])?;
            Ok(())
        })
        .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_lines_are_read_and_sorted() {
        let a = parse_line("com.x.cam\tcom.x.cam.Main\tCamera").unwrap();
        assert_eq!((a.pkg.as_str(), a.cls.as_str(), a.label.as_str()), ("com.x.cam", "com.x.cam.Main", "Camera"));
        assert_eq!(a.key(), "com.x.cam/com.x.cam.Main");
        assert!(parse_line("broken line").is_none());
        assert!(parse_line("\t\tNo package").is_none());
        let mut v = vec![parse_line("p.z\tc\tzebra").unwrap(), parse_line("p.a\tc\tApple").unwrap(), parse_line("p.b\tc\tbanana").unwrap(), parse_line("p.a2\tc\tApple").unwrap()];
        sort_apps(&mut v);
        let order: Vec<_> = v.iter().map(|a| a.pkg.as_str()).collect();
        assert_eq!(order, ["p.a", "p.a2", "p.b", "p.z"], "case-insensitive, ties by package");
    }

    #[test]
    fn android_pixels_become_an_image() {
        let px = [0xFF102030u32 as i32, 0x80FF0000u32 as i32, 0, 0xFFFFFFFFu32 as i32];
        let img = image_from_argb(&px, 2);
        assert_eq!(img.size, [2, 2]);
        assert_eq!((img.pixels[0].r(), img.pixels[0].g(), img.pixels[0].b(), img.pixels[0].a()), (0x10, 0x20, 0x30, 255));
        assert_eq!(img.pixels[1].a(), 0x80);
        assert_eq!(img.pixels[3].r(), 255);
    }
}
