package dev.heartnetapps.files;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import android.webkit.MimeTypeMap;

import java.io.File;
import java.io.FileNotFoundException;
import java.util.Locale;

/**
 * Lets other apps read the photos and videos the user chose to share. Not exported: only an
 * intent that carries a read grant (the share sheet) can open these links, and only for files
 * on shared storage, read-only.
 */
public final class ShareProvider extends ContentProvider {
    public static final String AUTHORITY = "dev.heartnetapps.files.share";

    public static Uri uriFor(String path) {
        String name = new File(path).getName();
        return new Uri.Builder().scheme("content").authority(AUTHORITY)
                .appendPath("f").appendPath(name).appendQueryParameter("p", path).build();
    }

    public static String mimeFor(String path) {
        String name = new File(path).getName();
        int dot = name.lastIndexOf('.');
        if (dot >= 0) {
            String m = MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substring(dot + 1).toLowerCase(Locale.ROOT));
            if (m != null) return m;
        }
        return "application/octet-stream";
    }

    private static File checked(Uri uri) throws FileNotFoundException {
        String p = uri.getQueryParameter("p");
        if (p == null) throw new FileNotFoundException("no file");
        try {
            File f = new File(p).getCanonicalFile();
            String c = f.getPath();
            boolean shared = c.startsWith("/storage/emulated/0/") || c.startsWith("/sdcard/");
            boolean appPrivate = c.contains("/Android/data/") || c.contains("/Android/obb/");
            boolean opened = false;
            try { opened = c.startsWith(new File(INSTANCE.getContext().getCacheDir(), "open").getCanonicalPath() + "/"); } catch (Throwable ignored) { }
            if (!((shared && !appPrivate) || opened) || !f.isFile()) throw new FileNotFoundException("not shareable");
            return f;
        } catch (java.io.IOException e) {
            throw new FileNotFoundException("bad path");
        }
    }

    private static ShareProvider INSTANCE;

    @Override public boolean onCreate() { INSTANCE = this; return true; }

    @Override public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (!"r".equals(mode)) throw new FileNotFoundException("read only");
        return ParcelFileDescriptor.open(checked(uri), ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override public Cursor query(Uri uri, String[] projection, String selection, String[] args, String sort) {
        File f;
        try { f = checked(uri); } catch (FileNotFoundException e) { return null; }
        String[] cols = projection != null ? projection : new String[] { OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE };
        MatrixCursor c = new MatrixCursor(cols, 1);
        Object[] row = new Object[cols.length];
        for (int i = 0; i < cols.length; i++) {
            if (OpenableColumns.DISPLAY_NAME.equals(cols[i])) row[i] = f.getName();
            else if (OpenableColumns.SIZE.equals(cols[i])) row[i] = f.length();
        }
        c.addRow(row);
        return c;
    }

    @Override public String getType(Uri uri) { return mimeFor(String.valueOf(uri.getQueryParameter("p"))); }
    @Override public Uri insert(Uri uri, ContentValues values) { throw new UnsupportedOperationException(); }
    @Override public int delete(Uri uri, String selection, String[] args) { throw new UnsupportedOperationException(); }
    @Override public int update(Uri uri, ContentValues values, String selection, String[] args) { throw new UnsupportedOperationException(); }
}
