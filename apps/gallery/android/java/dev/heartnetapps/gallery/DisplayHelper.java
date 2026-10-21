package dev.heartnetapps.gallery;

import android.app.Activity;
import android.view.Display;
import android.view.WindowManager;

/** Window-level refresh-rate request, the way games ask for the display's fastest mode. */
public final class DisplayHelper {
    private DisplayHelper() { }

    /** Safe to call from any thread; the change is applied on the UI thread. */
    public static void requestHighRefresh(final Activity activity) {
        activity.runOnUiThread(new Runnable() {
            @Override public void run() {
                try {
                    Display display = activity.getDisplay();
                    if (display == null) return;
                    Display.Mode current = display.getMode();
                    Display.Mode best = current;
                    for (Display.Mode m : display.getSupportedModes()) {
                        // same resolution, higher refresh rate
                        if (m.getPhysicalWidth() == current.getPhysicalWidth()
                                && m.getPhysicalHeight() == current.getPhysicalHeight()
                                && m.getRefreshRate() > best.getRefreshRate()) {
                            best = m;
                        }
                    }
                    WindowManager.LayoutParams lp = activity.getWindow().getAttributes();
                    if (lp.preferredDisplayModeId != best.getModeId()
                            || lp.preferredRefreshRate != best.getRefreshRate()) {
                        lp.preferredDisplayModeId = best.getModeId();
                        lp.preferredRefreshRate = best.getRefreshRate();
                        activity.getWindow().setAttributes(lp);
                    }
                } catch (Throwable ignored) { }
            }
        });
    }
}
