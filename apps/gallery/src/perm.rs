//! Android permission helpers, called through JNI on the running activity.

use android_activity::AndroidApp;
use jni::objects::{JObject, JValue};
use jni::{JNIEnv, JavaVM};

pub const MEDIA_IMAGES: &str = "android.permission.READ_MEDIA_IMAGES";
pub const MEDIA_VIDEO: &str = "android.permission.READ_MEDIA_VIDEO";
const PACKAGE: &str = "dev.heartnetapps.gallery";

fn with_env<R>(app: &AndroidApp, f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>) -> Option<R> {
    // SAFETY: both pointers come from android-activity and stay valid while the app runs;
    // JObject::from_raw does not take ownership of the activity reference.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let result = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
        return None;
    }
    result.ok()
}

pub fn has_permission(app: &AndroidApp, name: &str) -> bool {
    with_env(app, |env, act| {
        let s = env.new_string(name)?;
        env.call_method(act, "checkSelfPermission", "(Ljava/lang/String;)I", &[JValue::Object(&s)])?.i()
    })
    .is_some_and(|granted| granted == 0)
}

pub fn request_permissions(app: &AndroidApp, names: &[&str]) {
    with_env(app, |env, act| {
        let arr = env.new_object_array(names.len() as i32, "java/lang/String", JObject::null())?;
        for (i, name) in names.iter().enumerate() {
            let s = env.new_string(name)?;
            env.set_object_array_element(&arr, i as i32, &s)?;
        }
        env.call_method(act, "requestPermissions", "([Ljava/lang/String;I)V", &[JValue::Object(&arr), JValue::Int(7)])?;
        Ok(())
    });
}

/// Close the app (what Back does on the home screen of a normal Android app).
pub fn finish(app: &AndroidApp) {
    with_env(app, |env, act| {
        env.call_method(act, "finish", "()V", &[])?;
        Ok(())
    });
}

/// "All files access" is what lets the app move photos into its trash folder.
pub fn has_all_files_access(app: &AndroidApp) -> bool {
    with_env(app, |env, _| {
        env.call_static_method("android/os/Environment", "isExternalStorageManager", "()Z", &[])?.z()
    })
    .unwrap_or(false)
}

/// Ask for the display's fastest mode through the window attributes (what games do). Goes through
/// a small Java helper because those attributes must be changed on the UI thread, and the class is
/// looked up with the app's own class loader because a native thread cannot see app classes.
pub fn request_window_refresh(app: &AndroidApp) {
    with_env(app, |env, act| {
        let loader = env.call_method(act, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
        let name = env.new_string("dev.heartnetapps.gallery.DisplayHelper")?;
        let class = env
            .call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])?
            .l()?;
        let class = jni::objects::JClass::from(class);
        env.call_static_method(&class, "requestHighRefresh", "(Landroid/app/Activity;)V", &[JValue::Object(act)])?;
        Ok(())
    });
}

/// "Display over other apps", needed for the floating player.
pub fn can_draw_overlays(app: &AndroidApp) -> bool {
    with_env(app, |env, act| {
        env.call_static_method("android/provider/Settings", "canDrawOverlays", "(Landroid/content/Context;)Z", &[JValue::Object(act)])?.z()
    })
    .unwrap_or(false)
}

pub fn open_overlay_settings(app: &AndroidApp) {
    with_env(app, |env, act| {
        let uri_str = env.new_string(format!("package:{PACKAGE}"))?;
        let uri = env
            .call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&uri_str)])?
            .l()?;
        let action = env.new_string("android.settings.action.MANAGE_OVERLAY_PERMISSION")?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?;
        env.call_method(act, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    });
}

/// Start the floating player service (the Java class in this app) with a video file and start position.
pub fn start_floating(app: &AndroidApp, path: &str, pos_ms: i32) -> bool {
    with_env(app, |env, act| {
        let intent = env.new_object("android/content/Intent", "()V", &[])?;
        let pkg = env.new_string(PACKAGE)?;
        let cls = env.new_string("dev.heartnetapps.gallery.FloatingPlayerService")?;
        env.call_method(
            &intent,
            "setClassName",
            "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&pkg), JValue::Object(&cls)],
        )?;
        let (k_path, v_path) = (env.new_string("path")?, env.new_string(path)?);
        env.call_method(
            &intent,
            "putExtra",
            "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&k_path), JValue::Object(&v_path)],
        )?;
        let k_pos = env.new_string("pos")?;
        env.call_method(&intent, "putExtra", "(Ljava/lang/String;I)Landroid/content/Intent;", &[JValue::Object(&k_pos), JValue::Int(pos_ms)])?;
        env.call_method(act, "startForegroundService", "(Landroid/content/Intent;)Landroid/content/ComponentName;", &[JValue::Object(&intent)])?;
        Ok(())
    })
    .is_some()
}

pub fn move_task_to_back(app: &AndroidApp) {
    with_env(app, |env, act| {
        env.call_method(act, "moveTaskToBack", "(Z)Z", &[JValue::Bool(1)])?;
        Ok(())
    });
}

pub fn open_all_files_settings(app: &AndroidApp) {
    with_env(app, |env, act| {
        let uri_str = env.new_string(format!("package:{PACKAGE}"))?;
        let uri = env
            .call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&uri_str)])?
            .l()?;
        let action = env.new_string("android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION")?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?;
        env.call_method(act, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    });
}

/// Open the system share sheet for these files, through the app's own read-only content provider.
pub fn share_files(app: &AndroidApp, paths: &[String]) -> bool {
    with_env(app, |env, act| {
        let loader = env.call_method(act, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
        let name = env.new_string("dev.heartnetapps.gallery.ShareHelper")?;
        let class = env
            .call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])?
            .l()?;
        let class = jni::objects::JClass::from(class);
        let joined = env.new_string(paths.join("\n"))?;
        env.call_static_method(&class, "share", "(Landroid/app/Activity;Ljava/lang/String;)V", &[JValue::Object(act), JValue::Object(&joined)])?;
        Ok(())
    })
    .is_some()
}

fn helper<'a>(env: &mut JNIEnv<'a>, act: &JObject, class: &str) -> jni::errors::Result<jni::objects::JClass<'a>> {
    let loader = env.call_method(act, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
    let name = env.new_string(class)?;
    let c = env.call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])?.l()?;
    Ok(jni::objects::JClass::from(c))
}

const MEDIA_HELPER: &str = "dev.heartnetapps.gallery.MediaHelper";

pub fn set_brightness(app: &AndroidApp, f: f32) {
    with_env(app, |env, act| {
        let c = helper(env, act, MEDIA_HELPER)?;
        env.call_static_method(&c, "setBrightness", "(Landroid/app/Activity;F)V", &[JValue::Object(act), JValue::Float(f)])?;
        Ok(())
    });
}

pub fn get_brightness(app: &AndroidApp) -> Option<f32> {
    with_env(app, |env, act| {
        let c = helper(env, act, MEDIA_HELPER)?;
        env.call_static_method(&c, "getBrightness", "(Landroid/app/Activity;)F", &[JValue::Object(act)])?.f()
    })
}

pub fn get_volume(app: &AndroidApp) -> Option<f32> {
    with_env(app, |env, act| {
        let c = helper(env, act, MEDIA_HELPER)?;
        env.call_static_method(&c, "getVolume", "(Landroid/app/Activity;)F", &[JValue::Object(act)])?.f()
    })
}

pub fn set_volume(app: &AndroidApp, f: f32) -> Option<f32> {
    with_env(app, |env, act| {
        let c = helper(env, act, MEDIA_HELPER)?;
        env.call_static_method(&c, "setVolume", "(Landroid/app/Activity;F)F", &[JValue::Object(act), JValue::Float(f)])?.f()
    })
}

/// 0 = landscape, -1 = follow the sensor again.
pub fn set_orientation(app: &AndroidApp, orientation: i32) {
    with_env(app, |env, act| {
        let c = helper(env, act, MEDIA_HELPER)?;
        env.call_static_method(&c, "setOrientation", "(Landroid/app/Activity;I)V", &[JValue::Object(act), JValue::Int(orientation)])?;
        Ok(())
    });
}

/// While on, screenshots and the app-switcher preview are blocked.
pub fn set_secure(app: &AndroidApp, on: bool) {
    with_env(app, |env, act| {
        let c = helper(env, act, MEDIA_HELPER)?;
        env.call_static_method(&c, "setSecure", "(Landroid/app/Activity;Z)V", &[JValue::Object(act), JValue::Bool(on as u8)])?;
        Ok(())
    });
}

pub fn set_wallpaper(app: &AndroidApp, path: &str) {
    with_env(app, |env, act| {
        let c = helper(env, act, MEDIA_HELPER)?;
        let p = env.new_string(path)?;
        env.call_static_method(&c, "setWallpaper", "(Landroid/app/Activity;Ljava/lang/String;)V", &[JValue::Object(act), JValue::Object(&p)])?;
        Ok(())
    });
}

pub fn open_map(app: &AndroidApp, lat: f64, lon: f64) {
    with_env(app, |env, act| {
        let c = helper(env, act, MEDIA_HELPER)?;
        env.call_static_method(&c, "openMap", "(Landroid/app/Activity;DD)V", &[JValue::Object(act), JValue::Double(lat), JValue::Double(lon)])?;
        Ok(())
    });
}

/// Tell the system media database that these files appeared or are gone.
pub fn media_scan(app: &AndroidApp, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    with_env(app, |env, act| {
        let c = helper(env, act, MEDIA_HELPER)?;
        let p = env.new_string(paths.join("\n"))?;
        env.call_static_method(&c, "scan", "(Landroid/app/Activity;Ljava/lang/String;)V", &[JValue::Object(act), JValue::Object(&p)])?;
        Ok(())
    });
}

const ACTIVITY: &str = "dev.heartnetapps.gallery.GalleryActivity";

fn take_string(app: &AndroidApp, class: &str, method: &str) -> Option<String> {
    with_env(app, |env, act| {
        let c = helper(env, act, class)?;
        let o = env.call_static_method(&c, method, "()Ljava/lang/String;", &[])?.l()?;
        if o.is_null() {
            return Ok(None);
        }
        let s: String = env.get_string(&jni::objects::JString::from(o))?.into();
        Ok(Some(s))
    })
    .flatten()
}

/// A file another app opened with us, once.
pub fn take_view(app: &AndroidApp) -> Option<String> {
    take_string(app, ACTIVITY, "takeView")
}

/// A picker request from another app ("single;image/*"), once.
pub fn take_pick(app: &AndroidApp) -> Option<String> {
    take_string(app, ACTIVITY, "takePick")
}

pub fn finish_pick(app: &AndroidApp, paths: &[String]) {
    with_env(app, |env, act| {
        let c = helper(env, act, ACTIVITY)?;
        let p = env.new_string(paths.join("\n"))?;
        env.call_static_method(&c, "finishPick", "(Landroid/app/Activity;Ljava/lang/String;)V", &[JValue::Object(act), JValue::Object(&p)])?;
        Ok(())
    });
}

pub fn cancel_pick(app: &AndroidApp) {
    with_env(app, |env, act| {
        let c = helper(env, act, ACTIVITY)?;
        env.call_static_method(&c, "cancelPick", "(Landroid/app/Activity;)V", &[JValue::Object(act)])?;
        Ok(())
    });
}

const AUTH_HELPER: &str = "dev.heartnetapps.gallery.AuthHelper";

/// Show the system fingerprint / screen-lock prompt.
pub fn authenticate(app: &AndroidApp, title: &str) {
    with_env(app, |env, act| {
        let c = helper(env, act, AUTH_HELPER)?;
        let t = env.new_string(title)?;
        env.call_static_method(&c, "authenticate", "(Landroid/app/Activity;Ljava/lang/String;)V", &[JValue::Object(act), JValue::Object(&t)])?;
        Ok(())
    });
}

/// 0 still waiting, 1 confirmed, 2 cancelled or failed, 3 the phone has no screen lock.
pub fn take_auth(app: &AndroidApp) -> i32 {
    with_env(app, |env, act| {
        let c = helper(env, act, AUTH_HELPER)?;
        env.call_static_method(&c, "take", "()I", &[])?.i()
    })
    .unwrap_or(0)
}

/// Register the Rust function behind `GalleryActivity.nativeDestroying()`. (Registering by hand
/// is needed because the library is loaded by the native-activity glue, not by System.loadLibrary,
/// so Java's automatic name lookup would not find it.)
pub fn register_destroy_hook(app: &AndroidApp, hook: extern "system" fn(JNIEnv, jni::objects::JClass)) -> bool {
    with_env(app, |env, act| {
        let class = helper(env, act, ACTIVITY)?;
        env.register_native_methods(
            &class,
            &[jni::NativeMethod { name: "nativeDestroying".into(), sig: "()V".into(), fn_ptr: hook as *mut std::ffi::c_void }],
        )?;
        Ok(())
    })
    .is_some()
}
