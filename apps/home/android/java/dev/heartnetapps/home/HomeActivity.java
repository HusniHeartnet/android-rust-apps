package dev.heartnetapps.home;

import android.app.NativeActivity;

/**
 * The native activity plus one safeguard: the native glue's onDestroy waits for the app thread to
 * stop, but the windowing library never notices the destroy, so the Rust side is told here to close
 * its window loop, and if that is not enough the process is ended after two seconds (otherwise it
 * would be left stuck, and the next open would show only a blank splash screen).
 */
public class HomeActivity extends NativeActivity {
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
