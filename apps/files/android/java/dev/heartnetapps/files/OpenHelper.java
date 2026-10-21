package dev.heartnetapps.files;

import android.app.Activity;
import android.content.Intent;
import android.media.MediaScannerConnection;

/** Opening a file in another app, and telling Android's media database about changes. */
public final class OpenHelper {
    private OpenHelper() { }

    /** Open `path` with the app that handles its type (or the chooser). False when nothing can open it. */
    public static boolean open(Activity a, String path, boolean chooser) {
        try {
            Intent view = new Intent(Intent.ACTION_VIEW);
            view.setDataAndType(ShareProvider.uriFor(path), ShareProvider.mimeFor(path));
            view.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
            a.startActivity(chooser ? Intent.createChooser(view, "Open with") : view);
            return true;
        } catch (Throwable t) {
            return false;
        }
    }

    /** Paths separated by newlines. */
    public static void scan(Activity a, String joined) {
        try {
            String[] paths = joined.split("\n");
            if (paths.length > 0 && !paths[0].isEmpty()) {
                MediaScannerConnection.scanFile(a.getApplicationContext(), paths, null, null);
            }
        } catch (Throwable ignored) { }
    }
}
