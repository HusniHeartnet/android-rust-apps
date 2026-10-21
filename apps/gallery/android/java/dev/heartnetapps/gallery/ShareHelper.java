package dev.heartnetapps.gallery;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;

import java.util.ArrayList;

/** Opens the system share sheet for a list of files (paths separated by newlines). */
public final class ShareHelper {
    private ShareHelper() { }

    public static void share(final Activity activity, final String joinedPaths) {
        activity.runOnUiThread(new Runnable() {
            @Override public void run() {
                try {
                    String[] paths = joinedPaths.split("\n");
                    ArrayList<Uri> uris = new ArrayList<Uri>();
                    String type = null;
                    for (String p : paths) {
                        if (p.isEmpty()) continue;
                        uris.add(ShareProvider.uriFor(p));
                        String m = ShareProvider.mimeFor(p);
                        if (type == null) type = m;
                        else if (!type.equals(m)) {
                            String a = type.substring(0, type.indexOf('/'));
                            String b = m.substring(0, m.indexOf('/'));
                            type = a.equals(b) ? a + "/*" : "*/*";
                        }
                    }
                    if (uris.isEmpty()) return;
                    Intent send = new Intent(uris.size() == 1 ? Intent.ACTION_SEND : Intent.ACTION_SEND_MULTIPLE);
                    send.setType(type);
                    if (uris.size() == 1) send.putExtra(Intent.EXTRA_STREAM, uris.get(0));
                    else send.putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris);
                    send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                    activity.startActivity(Intent.createChooser(send, "Share"));
                } catch (Throwable ignored) { }
            }
        });
    }
}
