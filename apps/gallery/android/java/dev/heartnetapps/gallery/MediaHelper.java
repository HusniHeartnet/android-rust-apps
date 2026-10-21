package dev.heartnetapps.gallery;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.media.AudioManager;
import android.media.MediaScannerConnection;
import android.net.Uri;
import android.provider.Settings;
import android.view.WindowManager;

/** Small Android calls the Rust code cannot make directly (they need the UI thread or a class loader). */
public final class MediaHelper {
    private MediaHelper() { }

    /** 0..1, or a negative number to go back to the system's brightness. */
    public static void setBrightness(final Activity a, final float f) {
        a.runOnUiThread(new Runnable() {
            @Override public void run() {
                try {
                    WindowManager.LayoutParams lp = a.getWindow().getAttributes();
                    lp.screenBrightness = f < 0 ? WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE : Math.max(0.02f, Math.min(1f, f));
                    a.getWindow().setAttributes(lp);
                } catch (Throwable ignored) { }
            }
        });
    }

    public static float getBrightness(Activity a) {
        try {
            float b = a.getWindow().getAttributes().screenBrightness;
            if (b >= 0) return b;
            return Settings.System.getInt(a.getContentResolver(), Settings.System.SCREEN_BRIGHTNESS, 128) / 255f;
        } catch (Throwable t) {
            return 0.5f;
        }
    }

    public static float getVolume(Activity a) {
        try {
            AudioManager am = (AudioManager) a.getSystemService(Context.AUDIO_SERVICE);
            return am.getStreamVolume(AudioManager.STREAM_MUSIC) / (float) am.getStreamMaxVolume(AudioManager.STREAM_MUSIC);
        } catch (Throwable t) {
            return 0.5f;
        }
    }

    /** Sets the media volume (0..1) and returns what was applied. */
    public static float setVolume(Activity a, float f) {
        try {
            AudioManager am = (AudioManager) a.getSystemService(Context.AUDIO_SERVICE);
            int max = am.getStreamMaxVolume(AudioManager.STREAM_MUSIC);
            int idx = Math.round(Math.max(0f, Math.min(1f, f)) * max);
            am.setStreamVolume(AudioManager.STREAM_MUSIC, idx, 0);
            return idx / (float) max;
        } catch (Throwable t) {
            return f;
        }
    }

    /** An Android SCREEN_ORIENTATION_* value (0 landscape, -1 follow the sensor and the phone's setting). */
    public static void setOrientation(final Activity a, final int orientation) {
        a.runOnUiThread(new Runnable() {
            @Override public void run() {
                try { a.setRequestedOrientation(orientation); } catch (Throwable ignored) { }
            }
        });
    }

    /** While on, the screen cannot be captured and the app switcher shows a blank card. */
    public static void setSecure(final Activity a, final boolean on) {
        a.runOnUiThread(new Runnable() {
            @Override public void run() {
                try {
                    if (on) a.getWindow().addFlags(WindowManager.LayoutParams.FLAG_SECURE);
                    else a.getWindow().clearFlags(WindowManager.LayoutParams.FLAG_SECURE);
                } catch (Throwable ignored) { }
            }
        });
    }

    /** The system's "Set as" flow for a picture. */
    public static void setWallpaper(final Activity a, final String path) {
        a.runOnUiThread(new Runnable() {
            @Override public void run() {
                try {
                    Intent i = new Intent(Intent.ACTION_ATTACH_DATA);
                    i.setDataAndType(ShareProvider.uriFor(path), ShareProvider.mimeFor(path));
                    i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                    a.startActivity(Intent.createChooser(i, "Set as"));
                } catch (Throwable ignored) { }
            }
        });
    }

    public static void openMap(final Activity a, final double lat, final double lon) {
        a.runOnUiThread(new Runnable() {
            @Override public void run() {
                try {
                    a.startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse("geo:" + lat + "," + lon + "?q=" + lat + "," + lon)));
                } catch (Throwable ignored) { }
            }
        });
    }

    /** Tell the system's media database about files that appeared or disappeared (paths separated by newlines). */
    public static void scan(Activity a, String joined) {
        try {
            String[] paths = joined.split("\n");
            if (paths.length > 0 && !paths[0].isEmpty()) {
                MediaScannerConnection.scanFile(a.getApplicationContext(), paths, null, null);
            }
        } catch (Throwable ignored) { }
    }
}
