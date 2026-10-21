package dev.heartnetapps.files;

import android.app.NativeActivity;

/**
 * The native activity plus one safeguard. The native glue's onDestroy waits for the app thread to
 * stop, but the windowing library never notices the destroy, so when the activity is finished the
 * main thread would wait forever and the process would be left stuck (the next open would show
 * only a blank splash screen). The Rust side closes its window loop before finishing, which avoids
 * this; this class covers destroys that come from Android itself.
 */
public class FilesActivity extends NativeActivity {
    /** Implemented in Rust (registered at start-up). */
    private static native void nativeDestroying();

    @Override protected void onDestroy() {
        try {
            nativeDestroying();
        } catch (Throwable ignored) { }
        if (!isChangingConfigurations()) {
            new Thread(new Runnable() {
                @Override public void run() {
                    try { Thread.sleep(2000); } catch (InterruptedException ignored) { }
                    android.os.Process.killProcess(android.os.Process.myPid());
                }
            }).start();
        }
        super.onDestroy();
    }
}
