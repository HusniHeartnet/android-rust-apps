package dev.heartnetapps.gallery;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Matrix;
import android.graphics.Outline;
import android.graphics.Paint;
import android.graphics.PixelFormat;
import android.graphics.SurfaceTexture;
import android.graphics.drawable.Icon;
import android.media.AudioAttributes;
import android.media.MediaPlayer;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.provider.Settings;
import android.util.DisplayMetrics;
import android.util.Log;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.ScaleGestureDetector;
import android.view.Surface;
import android.view.TextureView;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.ViewOutlineProvider;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.widget.ImageButton;
import android.widget.LinearLayout;
import android.widget.SeekBar;

/**
 * A small video window that floats above other apps. Drag it anywhere, resize it to any size with
 * the corner handle or by pinching, no snapping. It never takes audio focus, so it keeps playing
 * next to Instagram, TikTok or YouTube.
 */
public class FloatingPlayerService extends Service implements TextureView.SurfaceTextureListener,
        MediaPlayer.OnPreparedListener, MediaPlayer.OnVideoSizeChangedListener,
        MediaPlayer.OnCompletionListener, MediaPlayer.OnErrorListener {

    private static final String TAG = "FloatingPlayer";
    private static final String CHANNEL = "floating_player";
    private static final String ACTION_STOP = "dev.heartnetapps.gallery.STOP_FLOAT";

    private WindowManager wm;
    private WindowManager.LayoutParams lp;
    private FrameLayout root;
    private TextureView textureView;
    private LinearLayout bar;
    private ImageButton playButton;
    private ImageButton muteButton;
    private ImageButton closeButton;
    private SeekBar seekBar;
    private HandleView handle;
    private MediaPlayer player;
    private Surface surface;
    private final Handler ui = new Handler(Looper.getMainLooper());
    private boolean prepared;
    private boolean muted;
    private boolean userSeeking;
    private int videoW = 16, videoH = 9;
    private String pendingPath;
    private int pendingPos;
    private float density;
    private int screenW, screenH;

    private final Runnable tick = new Runnable() {
        @Override public void run() {
            if (player != null && prepared && !userSeeking) {
                try {
                    seekBar.setProgress(player.getCurrentPosition());
                    playButton.setImageResource(player.isPlaying()
                            ? android.R.drawable.ic_media_pause : android.R.drawable.ic_media_play);
                } catch (IllegalStateException ignored) { }
            }
            ui.postDelayed(this, 400);
        }
    };

    private final Runnable hideControls = new Runnable() {
        @Override public void run() { showControls(false); }
    };

    @Override public IBinder onBind(Intent intent) { return null; }

    /** True while the floating player exists; the gallery's exit safety net leaves the process alone then. */
    public static volatile boolean running;

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        running = true;
        if (intent != null && ACTION_STOP.equals(intent.getAction())) {
            stopSelf();
            return START_NOT_STICKY;
        }
        startAsForeground();
        if (!Settings.canDrawOverlays(this)) {
            Log.w(TAG, "no overlay permission");
            stopSelf();
            return START_NOT_STICKY;
        }
        String path = intent == null ? null : intent.getStringExtra("path");
        int pos = intent == null ? 0 : intent.getIntExtra("pos", 0);
        if (path == null) {
            stopSelf();
            return START_NOT_STICKY;
        }
        if (root == null) createWindow();
        open(path, pos);
        return START_NOT_STICKY;
    }

    private void startAsForeground() {
        NotificationManager nm = (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
        nm.createNotificationChannel(new NotificationChannel(CHANNEL, "Floating video player",
                NotificationManager.IMPORTANCE_LOW));
        Intent stop = new Intent(this, FloatingPlayerService.class).setAction(ACTION_STOP);
        PendingIntent pi = PendingIntent.getService(this, 0, stop, PendingIntent.FLAG_IMMUTABLE);
        Notification n = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(android.R.drawable.ic_media_play)
                .setContentTitle("Floating video player")
                .setContentText("Playing above other apps")
                .addAction(new Notification.Action.Builder(
                        Icon.createWithResource(this, android.R.drawable.ic_menu_close_clear_cancel), "Close", pi).build())
                .build();
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(41, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
        } else {
            startForeground(41, n);
        }
    }

    private int dp(float v) { return Math.round(v * density); }

    private ImageButton button(int icon) {
        ImageButton b = new ImageButton(this);
        b.setImageResource(icon);
        b.setColorFilter(Color.WHITE);
        b.setBackgroundColor(Color.TRANSPARENT);
        b.setPadding(dp(8), dp(8), dp(8), dp(8));
        return b;
    }

    private void createWindow() {
        wm = (WindowManager) getSystemService(Context.WINDOW_SERVICE);
        DisplayMetrics dm = getResources().getDisplayMetrics();
        density = dm.density;
        screenW = dm.widthPixels;
        screenH = dm.heightPixels;

        root = new FrameLayout(this);
        root.setBackgroundColor(Color.BLACK);
        root.setClipToOutline(true);
        root.setOutlineProvider(new ViewOutlineProvider() {
            @Override public void getOutline(View view, Outline outline) {
                outline.setRoundRect(0, 0, view.getWidth(), view.getHeight(), dp(14));
            }
        });

        textureView = new TextureView(this);
        textureView.setSurfaceTextureListener(this);
        root.addView(textureView, new FrameLayout.LayoutParams(-1, -1));

        // bottom controls
        bar = new LinearLayout(this);
        bar.setOrientation(LinearLayout.HORIZONTAL);
        bar.setGravity(Gravity.CENTER_VERTICAL);
        bar.setBackgroundColor(Color.argb(150, 0, 0, 0));
        playButton = button(android.R.drawable.ic_media_pause);
        playButton.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { togglePlay(); }
        });
        seekBar = new SeekBar(this);
        seekBar.setOnSeekBarChangeListener(new SeekBar.OnSeekBarChangeListener() {
            @Override public void onProgressChanged(SeekBar s, int p, boolean fromUser) { }
            @Override public void onStartTrackingTouch(SeekBar s) { userSeeking = true; }
            @Override public void onStopTrackingTouch(SeekBar s) {
                userSeeking = false;
                if (player != null && prepared) player.seekTo(s.getProgress());
            }
        });
        muteButton = button(android.R.drawable.ic_lock_silent_mode_off);
        muteButton.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { toggleMute(); }
        });
        bar.addView(playButton, new LinearLayout.LayoutParams(dp(40), dp(40)));
        bar.addView(seekBar, new LinearLayout.LayoutParams(0, -2, 1f));
        bar.addView(muteButton, new LinearLayout.LayoutParams(dp(40), dp(40)));
        FrameLayout.LayoutParams barLp = new FrameLayout.LayoutParams(-1, dp(44), Gravity.BOTTOM);
        root.addView(bar, barLp);

        closeButton = button(android.R.drawable.ic_menu_close_clear_cancel);
        closeButton.setBackgroundColor(Color.argb(120, 0, 0, 0));
        closeButton.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { stopSelf(); }
        });
        root.addView(closeButton, new FrameLayout.LayoutParams(dp(38), dp(38), Gravity.TOP | Gravity.END));

        handle = new HandleView(this);
        root.addView(handle, new FrameLayout.LayoutParams(dp(44), dp(44), Gravity.BOTTOM | Gravity.END));

        root.setOnTouchListener(new DragResizeTouch());

        int w = Math.round(screenW * 0.62f);
        int h = Math.round(w * 9f / 16f);
        lp = new WindowManager.LayoutParams(w, h,
                WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE
                        | WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS
                        | WindowManager.LayoutParams.FLAG_HARDWARE_ACCELERATED,
                PixelFormat.TRANSLUCENT);
        lp.gravity = Gravity.TOP | Gravity.START;
        lp.x = screenW - w - dp(12);
        lp.y = dp(90);
        wm.addView(root, lp);
        showControls(true);
        ui.post(tick);
    }

    // ---------- playback ----------

    private void open(String path, int pos) {
        pendingPath = path;
        pendingPos = pos;
        if (surface != null) startPlayer();   // otherwise started once the surface exists
    }

    private void startPlayer() {
        releasePlayer();
        if (pendingPath == null || surface == null) return;
        try {
            player = new MediaPlayer();
            player.setAudioAttributes(new AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build());
            // Deliberately no audio-focus request: other apps keep playing and both are heard.
            player.setSurface(surface);
            player.setDataSource(pendingPath);
            player.setOnPreparedListener(this);
            player.setOnVideoSizeChangedListener(this);
            player.setOnCompletionListener(this);
            player.setOnErrorListener(this);
            if (muted) player.setVolume(0f, 0f);
            prepared = false;
            player.prepareAsync();
        } catch (Exception e) {
            Log.e(TAG, "cannot play " + pendingPath, e);
            stopSelf();
        }
    }

    private void releasePlayer() {
        prepared = false;
        if (player != null) {
            try { player.release(); } catch (Exception ignored) { }
            player = null;
        }
    }

    @Override public void onPrepared(MediaPlayer mp) {
        prepared = true;
        seekBar.setMax(mp.getDuration());
        if (pendingPos > 0) mp.seekTo(pendingPos);
        mp.start();
        videoW = Math.max(1, mp.getVideoWidth());
        videoH = Math.max(1, mp.getVideoHeight());
        fitVideo();
    }

    @Override public void onVideoSizeChanged(MediaPlayer mp, int w, int h) {
        if (w > 0 && h > 0) { videoW = w; videoH = h; fitVideo(); }
    }

    @Override public void onCompletion(MediaPlayer mp) {
        playButton.setImageResource(android.R.drawable.ic_media_play);
        showControls(true);
    }

    @Override public boolean onError(MediaPlayer mp, int what, int extra) {
        Log.e(TAG, "player error " + what + "/" + extra);
        stopSelf();
        return true;
    }

    private void togglePlay() {
        if (player == null || !prepared) return;
        if (player.isPlaying()) {
            player.pause();
        } else {
            if (player.getCurrentPosition() >= player.getDuration() - 300) player.seekTo(0);
            player.start();
        }
    }

    private void toggleMute() {
        muted = !muted;
        if (player != null) player.setVolume(muted ? 0f : 1f, muted ? 0f : 1f);
        muteButton.setImageResource(muted ? android.R.drawable.ic_lock_silent_mode
                : android.R.drawable.ic_lock_silent_mode_off);
    }

    /** Letterbox the video inside the window, whatever size the window has been dragged to. */
    private void fitVideo() {
        if (textureView == null) return;
        float vw = textureView.getWidth(), vh = textureView.getHeight();
        if (vw <= 0 || vh <= 0) return;
        float videoAr = (float) videoW / videoH, viewAr = vw / vh;
        float sx = 1f, sy = 1f;
        if (videoAr > viewAr) sy = (vw / videoAr) / vh; else sx = (vh * videoAr) / vw;
        Matrix m = new Matrix();
        m.setScale(sx, sy, vw / 2f, vh / 2f);
        textureView.setTransform(m);
    }

    private void showControls(boolean show) {
        ui.removeCallbacks(hideControls);
        int vis = show ? View.VISIBLE : View.GONE;
        bar.setVisibility(vis);
        closeButton.setVisibility(vis);
        handle.setAlpha(show ? 1f : 0.35f);
        if (show) ui.postDelayed(hideControls, 3500);
    }

    // ---------- TextureView callbacks ----------

    @Override public void onSurfaceTextureAvailable(SurfaceTexture st, int w, int h) {
        surface = new Surface(st);
        if (pendingPath != null) startPlayer();
    }

    @Override public void onSurfaceTextureSizeChanged(SurfaceTexture st, int w, int h) { fitVideo(); }

    @Override public boolean onSurfaceTextureDestroyed(SurfaceTexture st) {
        releasePlayer();
        if (surface != null) { surface.release(); surface = null; }
        return true;
    }

    @Override public void onSurfaceTextureUpdated(SurfaceTexture st) { }

    // ---------- touch: drag, corner resize, pinch resize (free aspect, no snapping) ----------

    private class DragResizeTouch implements View.OnTouchListener {
        private final ScaleGestureDetector pinch;
        private final int slop = ViewConfiguration.get(FloatingPlayerService.this).getScaledTouchSlop();
        private float downX, downY;
        private int startX, startY, startW, startH;
        private boolean resizing, moved;

        DragResizeTouch() {
            pinch = new ScaleGestureDetector(FloatingPlayerService.this, new ScaleGestureDetector.SimpleOnScaleGestureListener() {
                @Override public boolean onScale(ScaleGestureDetector d) {
                    float px = d.getPreviousSpanX(), py = d.getPreviousSpanY();
                    float fx = px > 8 ? d.getCurrentSpanX() / px : 1f;
                    float fy = py > 8 ? d.getCurrentSpanY() / py : 1f;
                    resizeAround(fx, fy);
                    moved = true;
                    return true;
                }
            });
        }

        @Override public boolean onTouch(View v, MotionEvent e) {
            pinch.onTouchEvent(e);
            if (pinch.isInProgress() || e.getPointerCount() > 1) return true;
            switch (e.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    downX = e.getRawX(); downY = e.getRawY();
                    startX = lp.x; startY = lp.y; startW = lp.width; startH = lp.height;
                    resizing = e.getX() > v.getWidth() - dp(44) && e.getY() > v.getHeight() - dp(44);
                    moved = false;
                    return true;
                case MotionEvent.ACTION_MOVE: {
                    float dx = e.getRawX() - downX, dy = e.getRawY() - downY;
                    if (!moved && Math.abs(dx) < slop && Math.abs(dy) < slop) return true;
                    moved = true;
                    if (resizing) {
                        lp.width = clamp(Math.round(startW + dx), dp(120), screenW * 2);
                        lp.height = clamp(Math.round(startH + dy), dp(80), screenH * 2);
                    } else {
                        // keep at least a grab-able corner on screen, otherwise move freely
                        lp.x = clamp(Math.round(startX + dx), -lp.width + dp(60), screenW - dp(60));
                        lp.y = clamp(Math.round(startY + dy), -lp.height + dp(60), screenH - dp(60));
                    }
                    wm.updateViewLayout(root, lp);
                    return true;
                }
                case MotionEvent.ACTION_UP:
                    if (!moved) showControls(bar.getVisibility() != View.VISIBLE);
                    else showControls(true);
                    return true;
                default:
                    return true;
            }
        }
    }

    private void resizeAround(float fx, float fy) {
        int newW = clamp(Math.round(lp.width * fx), dp(120), screenW * 2);
        int newH = clamp(Math.round(lp.height * fy), dp(80), screenH * 2);
        lp.x += (lp.width - newW) / 2;
        lp.y += (lp.height - newH) / 2;
        lp.width = newW;
        lp.height = newH;
        wm.updateViewLayout(root, lp);
    }

    private static int clamp(int v, int lo, int hi) { return Math.max(lo, Math.min(hi, v)); }

    /** The little grip in the bottom-right corner. */
    private static class HandleView extends View {
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        HandleView(Context c) {
            super(c);
            paint.setColor(Color.WHITE);
            paint.setStrokeWidth(3f * c.getResources().getDisplayMetrics().density);
            paint.setStrokeCap(Paint.Cap.ROUND);
        }
        @Override protected void onDraw(Canvas canvas) {
            float w = getWidth(), h = getHeight(), p = w * 0.22f;
            canvas.drawLine(w - p, h - p * 3.2f, w - p * 3.2f, h - p, paint);
            canvas.drawLine(w - p, h - p * 2.0f, w - p * 2.0f, h - p, paint);
            canvas.drawLine(w - p, h - p * 0.8f, w - p * 0.8f, h - p, paint);
        }
    }

    @Override public void onDestroy() {
        running = false;
        ui.removeCallbacksAndMessages(null);
        releasePlayer();
        if (root != null && wm != null) {
            try { wm.removeView(root); } catch (Exception ignored) { }
            root = null;
        }
        if (surface != null) { surface.release(); surface = null; }
        super.onDestroy();
    }
}
