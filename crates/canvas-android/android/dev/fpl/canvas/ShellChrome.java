package dev.fpl.canvas;

import android.content.res.ColorStateList;
import android.graphics.*;
import android.graphics.drawable.Drawable;
import android.view.View;
import android.widget.Button;
import org.json.JSONObject;

/** Raster visitor for shared GFX chrome. Rust owns geometry and visual state. */
final class ShellChrome {
    private static native int[] nativeMetrics();
    private static final class Metrics {
        static final int[] VALUES=nativeMetrics();
        static { if(VALUES==null || VALUES.length!=4 || VALUES[0]<32 || VALUES[0]>64 || VALUES[1]<32 || VALUES[1]>64 || VALUES[2]<10 || VALUES[2]>24 || VALUES[3]<0 || VALUES[3]>16) throw new IllegalStateException("Invalid shared shell metrics"); }
    }
    static int barHeight() { return Metrics.VALUES[0]; }
    static int controlHeight() { return Metrics.VALUES[1]; }
    static int textSize() { return Metrics.VALUES[2]; }
    static int barPadding() { return Metrics.VALUES[3]; }
    static final int AUDIO_WIDTH_DP=88, AUDIO_HEIGHT_DP=44, AUDIO_INSET_DP=6;
    private static native float[] nativeFrame(float width, float height, int state, boolean bar, boolean quiet);
    private static int color(JSONObject palette, String role) throws Exception {
        return Color.parseColor(palette.getString(role));
    }
    private static int ink(JSONObject palette, int state, boolean quiet) throws Exception {
        float[] frame = nativeFrame(1,1,state,false,quiet);
        if(frame == null || frame.length != 10) throw new IllegalStateException("Invalid GFX chrome ink");
        if(frame[8] == 4) return color(palette,"text");
        if(frame[8] == 5) return color(palette,"text_muted");
        throw new IllegalStateException("Unsupported GFX chrome label role");
    }
    private static final class Face extends Drawable {
        private final float density;
        private final boolean bar, quiet;
        private final int[] colors = new int[16];
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private int opacity = 255;
        private float[] records;
        Face(View view, JSONObject palette, boolean bar, boolean quiet) throws Exception {
            density = view.getResources().getDisplayMetrics().density;
            this.bar = bar;
            this.quiet = quiet;
            colors[1] = color(palette,"surface");
            colors[2] = color(palette,"surface_muted");
            colors[3] = color(palette,"background_strong");
            colors[4] = color(palette,"text");
            colors[5] = color(palette,"text_muted");
            colors[6] = color(palette,"primary");
            colors[13] = color(palette,"border");
        }
        @Override public boolean isStateful() { return !bar; }
        @Override protected boolean onStateChange(int[] states) { records = null; invalidateSelf(); return true; }
        @Override protected void onBoundsChange(Rect bounds) { records = null; }
        @Override public void draw(Canvas canvas) {
            Rect bounds = getBounds();
            if(bounds.width() <= 0 || bounds.height() <= 0) return;
            if(records == null) {
                int state = 0;
                boolean enabled = false;
                for(int value : getState()) {
                    if(value == android.R.attr.state_enabled) enabled = true;
                    if(value == android.R.attr.state_focused) state |= 2;
                    if(value == android.R.attr.state_pressed) state |= 4;
                    if(value == android.R.attr.state_selected) state |= 1;
                }
                if(!enabled) state |= 8;
                records = nativeFrame(bounds.width()/density, bounds.height()/density, state, bar, quiet);
                if(records == null || records.length != 10) throw new IllegalStateException("Invalid GFX chrome frame");
            }
            float stroke = records[5]*density;
            RectF rect = new RectF(bounds.left + records[0]*density + stroke/2,
                bounds.top + records[1]*density + stroke/2,
                bounds.left + (records[0]+records[2])*density - stroke/2,
                bounds.top + (records[1]+records[3])*density - stroke/2);
            paint.setStyle(Paint.Style.FILL); paint.setColor(colors[(int)records[6]]);
            paint.setAlpha(Math.round(opacity*records[9]));
            canvas.drawRoundRect(rect,records[4]*density,records[4]*density,paint);
            if(stroke > 0) {
                paint.setStyle(Paint.Style.STROKE); paint.setStrokeWidth(stroke);
                paint.setColor(colors[(int)records[7]]);
                paint.setAlpha(Math.round(opacity*records[9]));
                canvas.drawRoundRect(rect,records[4]*density,records[4]*density,paint);
            }
        }
        @Override public void setAlpha(int alpha) { opacity = Math.max(0,Math.min(255,alpha)); invalidateSelf(); }
        @Override public void setColorFilter(ColorFilter filter) { paint.setColorFilter(filter); invalidateSelf(); }
        @Override public int getOpacity() { return PixelFormat.TRANSLUCENT; }
    }
    static void control(Button button, JSONObject palette, boolean selected) throws Exception {
        control(button,palette,selected,false);
    }
    static void control(Button button, JSONObject palette, boolean selected, boolean quiet) throws Exception {
        button.setBackgroundTintList(null);
        button.setBackground(new Face(button,palette,false,quiet));
        button.setTextColor(new ColorStateList(new int[][]{
            {-android.R.attr.state_enabled}, {android.R.attr.state_pressed},
            {android.R.attr.state_focused}, {android.R.attr.state_selected}, {}},
            new int[]{ink(palette,8,quiet),ink(palette,4,quiet),ink(palette,2,quiet),
                ink(palette,1,quiet),ink(palette,0,quiet)}));
        button.setAllCaps(false);
        button.setTypeface(Typeface.create("sans-serif-medium",Typeface.NORMAL));
        button.setIncludeFontPadding(false);
        button.setElevation(0); button.setStateListAnimator(null);
        button.setMinWidth(0); button.setMinimumWidth(0);
        button.setMinHeight(0); button.setMinimumHeight(0);
        int padding=Math.round(14*button.getResources().getDisplayMetrics().density);
        button.setPadding(padding,0,padding,0);
        button.setOnFocusChangeListener((view,focused) -> TvMotion.focus(view,focused));
        button.setSelected(selected);
    }
    static void bar(View view, JSONObject palette) throws Exception {
        view.setBackground(new Face(view,palette,true,false));
    }
    /** Source-local action, not device-wide volume or proof of audible signal. */
    static void audioState(Button button, String title, boolean muted) {
        button.setText(audioAction(muted));
        button.setTextSize(14);
        button.setContentDescription(audioDescription(title,muted));
    }
    static String audioAction(boolean muted) { return muted ? "Unmute" : "Mute"; }
    static String audioDescription(String title, boolean muted) {
        return audioAction(muted) + " " + title + " audio. This stream is " +
            (muted ? "muted." : "unmuted.");
    }
    private ShellChrome() {}
}
