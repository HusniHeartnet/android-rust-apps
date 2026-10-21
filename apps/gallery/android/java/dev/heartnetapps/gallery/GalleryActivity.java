package dev.heartnetapps.gallery;

import android.app.Activity;
import android.app.NativeActivity;
import android.content.ClipData;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.webkit.MimeTypeMap;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * The app's one activity. It is the native activity plus the part that listens to other apps:
 * "Open with Gallery" for a photo or video, and the picker other apps use to ask for one.
 * The Rust side collects what arrives through the static take... methods.
 */
public class GalleryActivity extends NativeActivity {
    private static volatile String pendingView;
    private static volatile String pendingPick;

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        handle(getIntent());
    }

    /**
     * Implemented in Rust (registered at start-up). The native glue's onDestroy waits for the app
     * thread to stop, but the windowing library never notices the destroy, so without this the
     * main thread would wait forever and the process would be left stuck: the next time the app
     * is opened it would show only a blank splash screen.
     */
    private static native void nativeDestroying();

    @Override protected void onDestroy() {
        try {
            nativeDestroying();
        } catch (Throwable ignored) { }
        if (!isChangingConfigurations() && !FloatingPlayerService.running) {
            // If the app thread still has not stopped soon, the process would be left stuck and the
            // next launch would be a blank splash screen: end it instead.
            new Thread(new Runnable() {
                @Override public void run() {
                    try { Thread.sleep(2000); } catch (InterruptedException ignored) { }
                    if (!FloatingPlayerService.running) android.os.Process.killProcess(android.os.Process.myPid());
                }
            }).start();
        }
        super.onDestroy();
    }

    @Override protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        handle(intent);
    }

    private void handle(final Intent intent) {
        if (intent == null) return;
        String action = intent.getAction();
        if (Intent.ACTION_VIEW.equals(action) && intent.getData() != null) {
            final Uri uri = intent.getData();
            new Thread(new Runnable() {
                @Override public void run() {
                    String path = resolve(uri);
                    if (path != null) pendingView = path;
                }
            }).start();
        } else if (Intent.ACTION_GET_CONTENT.equals(action) || Intent.ACTION_PICK.equals(action)) {
            boolean multiple = intent.getBooleanExtra(Intent.EXTRA_ALLOW_MULTIPLE, false);
            String type = intent.getType();
            if (type == null && intent.getData() != null) {
                String d = intent.getData().toString();
                type = d.contains("video") ? "video/*" : d.contains("images") ? "image/*" : "*/*";
            }
            pendingPick = (multiple ? "multiple;" : "single;") + (type == null ? "*/*" : type);
        }
    }

    /** A real file path for a link another app sent: its own path when it has one, else a copy in our cache. */
    private String resolve(Uri uri) {
        String scheme = uri.getScheme();
        if ("file".equals(scheme)) return uri.getPath();
        if (!"content".equals(scheme)) return null;
        try {
            Cursor c = getContentResolver().query(uri, new String[] { "_data" }, null, null, null);
            if (c != null) {
                try {
                    if (c.moveToFirst() && !c.isNull(0)) {
                        File f = new File(c.getString(0));
                        if (f.isFile()) return f.getPath();
                    }
                } finally { c.close(); }
            }
        } catch (Throwable ignored) { }
        try {
            String name = "shared";
            Cursor c = getContentResolver().query(uri, new String[] { OpenableColumns.DISPLAY_NAME }, null, null, null);
            if (c != null) {
                try { if (c.moveToFirst() && !c.isNull(0)) name = c.getString(0); } finally { c.close(); }
            }
            name = name.replace('/', '_');
            if (name.indexOf('.') < 0) {
                String ext = MimeTypeMap.getSingleton().getExtensionFromMimeType(getContentResolver().getType(uri));
                if (ext != null) name += "." + ext;
            }
            File dir = new File(getCacheDir(), "open");
            dir.mkdirs();
            long cutoff = System.currentTimeMillis() - 24L * 3600 * 1000;
            File[] old = dir.listFiles();
            if (old != null) for (File f : old) if (f.lastModified() < cutoff) f.delete();
            File out = new File(dir, name);
            InputStream in = getContentResolver().openInputStream(uri);
            if (in == null) return null;
            try {
                OutputStream os = new FileOutputStream(out);
                try {
                    byte[] buf = new byte[256 * 1024];
                    int n;
                    while ((n = in.read(buf)) > 0) os.write(buf, 0, n);
                } finally { os.close(); }
            } finally { in.close(); }
            return out.getPath();
        } catch (Throwable t) {
            return null;
        }
    }

    /** A file another app asked us to show (null when there is none), handed over once. */
    public static String takeView() {
        String p = pendingView;
        pendingView = null;
        return p;
    }

    /** "single" or "multiple", then ";" and the wanted type, when another app asked for a picture; handed over once. */
    public static String takePick() {
        String p = pendingPick;
        pendingPick = null;
        return p;
    }

    /** Answer the picker request with these files (paths separated by newlines) and close. */
    public static void finishPick(final Activity a, final String joined) {
        a.runOnUiThread(new Runnable() {
            @Override public void run() {
                try {
                    Intent result = new Intent();
                    ClipData clip = null;
                    Uri first = null;
                    for (String p : joined.split("\n")) {
                        if (p.isEmpty()) continue;
                        Uri u = ShareProvider.uriFor(p);
                        if (first == null) first = u;
                        if (clip == null) clip = ClipData.newUri(a.getContentResolver(), "media", u);
                        else clip.addItem(new ClipData.Item(u));
                    }
                    if (first != null) {
                        result.setData(first);
                        result.setClipData(clip);
                        result.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                        a.setResult(Activity.RESULT_OK, result);
                    } else {
                        a.setResult(Activity.RESULT_CANCELED);
                    }
                    // not finished here: the app closes its window loop first (see Gallery::exit),
                    // and finishing from there avoids leaving the process stuck
                } catch (Throwable ignored) { }
            }
        });
    }

    public static void cancelPick(final Activity a) {
        a.runOnUiThread(new Runnable() {
            @Override public void run() {
                a.setResult(Activity.RESULT_CANCELED);
            }
        });
    }
}
