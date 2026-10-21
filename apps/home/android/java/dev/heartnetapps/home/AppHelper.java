package dev.heartnetapps.home;

import android.app.Activity;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.drawable.Drawable;
import android.net.Uri;
import android.provider.Settings;

import java.util.ArrayList;
import java.util.List;

/** What a launcher needs from Android: the apps that can be started, their icons, and starting one. */
public final class AppHelper {
    private AppHelper() { }

    /** One line per app: package, TAB, activity, TAB, label. This launcher itself is left out. */
    public static String[] listApps(Activity a) {
        PackageManager pm = a.getPackageManager();
        Intent main = new Intent(Intent.ACTION_MAIN);
        main.addCategory(Intent.CATEGORY_LAUNCHER);
        List<ResolveInfo> found = pm.queryIntentActivities(main, 0);
        ArrayList<String> lines = new ArrayList<String>();
        for (ResolveInfo r : found) {
            if (r.activityInfo == null || a.getPackageName().equals(r.activityInfo.packageName)) continue;
            String label = String.valueOf(r.loadLabel(pm)).replace('\t', ' ').replace('\n', ' ');
            lines.add(r.activityInfo.packageName + "\t" + r.activityInfo.name + "\t" + label);
        }
        return lines.toArray(new String[0]);
    }

    /** The app's icon drawn into px by px ARGB pixels (adaptive icons included). */
    public static int[] icon(Activity a, String pkg, String cls, int px) {
        int[] pixels = new int[px * px];
        try {
            Drawable d = a.getPackageManager().getActivityIcon(new ComponentName(pkg, cls));
            Bitmap b = Bitmap.createBitmap(px, px, Bitmap.Config.ARGB_8888);
            Canvas c = new Canvas(b);
            d.setBounds(0, 0, px, px);
            d.draw(c);
            b.getPixels(pixels, 0, px, 0, 0, px, px);
            b.recycle();
        } catch (Throwable ignored) { }
        return pixels;
    }

    public static boolean launch(Activity a, String pkg, String cls) {
        try {
            Intent i = new Intent(Intent.ACTION_MAIN);
            i.addCategory(Intent.CATEGORY_LAUNCHER);
            i.setComponent(new ComponentName(pkg, cls));
            i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_RESET_TASK_IF_NEEDED);
            a.startActivity(i);
            return true;
        } catch (Throwable t) {
            return false;
        }
    }

    /** The system's settings page for an app (storage, permissions, uninstall). */
    public static void appInfo(Activity a, String pkg) {
        try {
            Intent i = new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:" + pkg));
            i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            a.startActivity(i);
        } catch (Throwable ignored) { }
    }
}
