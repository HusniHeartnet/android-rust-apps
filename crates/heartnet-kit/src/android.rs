//! Android calls made through JNI on the running activity, and the exit-hang fix.

use android_activity::AndroidApp;
use jni::objects::{JClass, JObject, JValue};
use jni::{JNIEnv, JavaVM};
use std::path::PathBuf;
use std::sync::Mutex;

/// Every app uses this activity class (from `java/dev/heartnetapps/kit`), so the hook is registered by name.
const ACTIVITY_CLASS: &str = "dev.heartnetapps.kit.KitActivity";

/// The window loop's context, so the destroy hook (called from Java on the main thread) can end it.
static CTX: Mutex<Option<egui::Context>> = Mutex::new(None);

pub fn with_env<R>(app: &AndroidApp, f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>) -> Option<R> {
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

/// Load one of the app's own Java classes (the native thread's default loader cannot see them).
pub fn class<'a>(env: &mut JNIEnv<'a>, act: &JObject, name: &str) -> jni::errors::Result<JClass<'a>> {
    let loader = env.call_method(act, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
    let n = env.new_string(name)?;
    let c = env.call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&n)])?.l()?;
    Ok(JClass::from(c))
}

/// Called by `KitActivity.onDestroy` just before Android waits for the app thread to stop.
/// The windowing library ignores Android's destroy notice, so ask the window to close here.
extern "system" fn native_destroying(_env: JNIEnv, _class: JClass) {
    if let Some(ctx) = CTX.lock().ok().and_then(|g| g.clone()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }
}

/// Set up the exit fix. Call once, when the app is created.
pub fn install_lifecycle(app: &AndroidApp, ctx: &egui::Context) -> bool {
    *CTX.lock().unwrap() = Some(ctx.clone());
    let ok = with_env(app, |env, act| {
        let class = class(env, act, ACTIVITY_CLASS)?;
        env.register_native_methods(
            &class,
            &[jni::NativeMethod { name: "nativeDestroying".into(), sig: "()V".into(), fn_ptr: native_destroying as *mut std::ffi::c_void }],
        )?;
        Ok(())
    })
    .is_some();
    crate::diag::log(&format!("destroy hook registered: {ok}"));
    ok
}

pub fn has_permission(app: &AndroidApp, name: &str) -> bool {
    with_env(app, |env, act| {
        let s = env.new_string(name)?;
        env.call_method(act, "checkSelfPermission", "(Ljava/lang/String;)I", &[JValue::Object(&s)])?.i()
    })
    .is_some_and(|granted| granted == 0)
}

pub fn request_permission(app: &AndroidApp, name: &str) {
    with_env(app, |env, act| {
        let arr = env.new_object_array(1, "java/lang/String", JObject::null())?;
        let s = env.new_string(name)?;
        env.set_object_array_element(&arr, 0, &s)?;
        env.call_method(act, "requestPermissions", "([Ljava/lang/String;I)V", &[JValue::Object(&arr), JValue::Int(7)])?;
        Ok(())
    });
}

/// The app's private folder (no permission needed, removed when the app is uninstalled).
pub fn files_dir(app: &AndroidApp) -> PathBuf {
    app.internal_data_path().unwrap_or_else(|| PathBuf::from("/data/local/tmp"))
}

/// Show or hide the on-screen keyboard.
pub fn soft_keyboard(app: &AndroidApp, show: bool) {
    if show {
        app.show_soft_input(true);
    } else {
        app.hide_soft_input(true);
    }
}

/// The (top, bottom) space in points that the status and navigation bars cover.
pub fn insets(app: &AndroidApp, ctx: &egui::Context) -> (f32, f32) {
    const FALLBACK: (f32, f32) = (30.0, 36.0);
    let r = app.content_rect();
    let ppp = ctx.pixels_per_point();
    let win_h = ctx.viewport_rect().height() * ppp;
    let top = r.top.max(0) as f32 / ppp;
    let bottom = (win_h - r.bottom as f32).max(0.0) / ppp;
    if top + bottom > 0.5 { (top, bottom) } else { FALLBACK }
}
