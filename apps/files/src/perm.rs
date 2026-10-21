//! Android calls made through JNI on the running activity.

use android_activity::AndroidApp;
use jni::objects::{JClass, JObject, JValue};
use jni::{JNIEnv, JavaVM};

const PACKAGE: &str = "dev.heartnetapps.files";
const ACTIVITY: &str = "dev.heartnetapps.files.FilesActivity";
const HELPER: &str = "dev.heartnetapps.files.OpenHelper";

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

fn class<'a>(env: &mut JNIEnv<'a>, act: &JObject, name: &str) -> jni::errors::Result<JClass<'a>> {
    let loader = env.call_method(act, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
    let n = env.new_string(name)?;
    let c = env.call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&n)])?.l()?;
    Ok(JClass::from(c))
}

/// "All files access" is what lets the app see and change everything on the phone's storage.
pub fn has_all_files_access(app: &AndroidApp) -> bool {
    with_env(app, |env, _| env.call_static_method("android/os/Environment", "isExternalStorageManager", "()Z", &[])?.z()).unwrap_or(false)
}

pub fn open_all_files_settings(app: &AndroidApp) {
    with_env(app, |env, act| {
        let uri_str = env.new_string(format!("package:{PACKAGE}"))?;
        let uri = env.call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&uri_str)])?.l()?;
        let action = env.new_string("android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION")?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;Landroid/net/Uri;)V", &[JValue::Object(&action), JValue::Object(&uri)])?;
        env.call_method(act, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    });
}

/// Open a file with whatever app handles its type. False when no app does.
pub fn open_file(app: &AndroidApp, path: &str, chooser: bool) -> bool {
    with_env(app, |env, act| {
        let c = class(env, act, HELPER)?;
        let p = env.new_string(path)?;
        env.call_static_method(&c, "open", "(Landroid/app/Activity;Ljava/lang/String;Z)Z", &[JValue::Object(act), JValue::Object(&p), JValue::Bool(chooser as u8)])?.z()
    })
    .unwrap_or(false)
}

/// The system share sheet for these files.
pub fn share_files(app: &AndroidApp, paths: &[String]) -> bool {
    with_env(app, |env, act| {
        let c = class(env, act, "dev.heartnetapps.files.ShareHelper")?;
        let joined = env.new_string(paths.join("\n"))?;
        env.call_static_method(&c, "share", "(Landroid/app/Activity;Ljava/lang/String;)V", &[JValue::Object(act), JValue::Object(&joined)])?;
        Ok(())
    })
    .is_some()
}

/// Tell Android's media database about files that appeared or disappeared, so galleries stay right.
pub fn media_scan(app: &AndroidApp, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    with_env(app, |env, act| {
        let c = class(env, act, HELPER)?;
        let p = env.new_string(paths.join("\n"))?;
        env.call_static_method(&c, "scan", "(Landroid/app/Activity;Ljava/lang/String;)V", &[JValue::Object(act), JValue::Object(&p)])?;
        Ok(())
    });
}

/// Register the Rust function behind `FilesActivity.nativeDestroying()`. (By hand, because the
/// library is loaded by the native-activity glue, so Java's name lookup would not find it.)
pub fn register_destroy_hook(app: &AndroidApp, hook: extern "system" fn(JNIEnv, JClass)) -> bool {
    with_env(app, |env, act| {
        let c = class(env, act, ACTIVITY)?;
        env.register_native_methods(&c, &[jni::NativeMethod { name: "nativeDestroying".into(), sig: "()V".into(), fn_ptr: hook as *mut std::ffi::c_void }])?;
        Ok(())
    })
    .is_some()
}
