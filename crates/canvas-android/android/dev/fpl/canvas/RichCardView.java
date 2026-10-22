package dev.fpl.canvas;

import android.content.Context;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.view.Gravity;
import android.view.KeyEvent;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import org.json.JSONArray;
import org.json.JSONObject;

/** Native adapter for the shared rich_card primitive; content stays owner-authored. */
final class RichCardView extends LinearLayout {
    private final TextView heading;
    private final TextView body;
    private final TextView footer;
    RichCardView(Context context, JSONObject spec, JSONObject palette) throws Exception {
        super(context);
        setOrientation(VERTICAL);
        int pad = dp(12);
        setPadding(pad, pad, pad, pad);
        int foreground = Color.parseColor(palette.getString("text"));
        int muted = Color.parseColor(palette.getString("text_muted"));
        GradientDrawable shape = new GradientDrawable();
        shape.setColor(Color.parseColor(palette.getString("surface")));
        shape.setCornerRadius(dp(10));
        shape.setStroke(dp(1), Color.parseColor(palette.getString("border")));
        setBackground(shape);

        heading = new TextView(context);
        heading.setText(spec.getString("title"));
        heading.setTextColor(foreground);
        heading.setTextSize(17);
        heading.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
        addView(heading, new LayoutParams(-1, -2));

        String format = spec.getString("format");
        boolean code = format.equals("code") || format.equals("diff");
        body = new TextView(context);
        body.setText(spec.getString("body"));
        body.setTextColor(format.equals("diff") ? Color.parseColor(palette.getString("info")) : foreground);
        body.setTextSize(code ? 15 : 14);
        body.setTypeface(code ? Typeface.MONOSPACE : Typeface.DEFAULT);
        body.setPadding(0, dp(8), 0, dp(8));
        body.setGravity(Gravity.TOP | Gravity.LEFT);
        ScrollView scroll = new ScrollView(context);
        scroll.setFocusable(true);
        scroll.setFocusableInTouchMode(true);
        scroll.setFillViewport(true);
        scroll.addView(body, new ScrollView.LayoutParams(-1, -2));
        scroll.setOnKeyListener((view, key, event) -> {
            if (event.getAction() != KeyEvent.ACTION_DOWN) return false;
            if (key != KeyEvent.KEYCODE_DPAD_UP && key != KeyEvent.KEYCODE_DPAD_DOWN) return false;
            int direction = key == KeyEvent.KEYCODE_DPAD_UP ? -1 : 1;
            int before = scroll.getScrollY();
            scroll.smoothScrollBy(0, direction * dp(54));
            // At the boundary let Android move focus to other workspace controls.
            return direction < 0 ? before > 0 : scroll.canScrollVertically(1);
        });
        addView(scroll, new LayoutParams(-1, 0, 1));

        JSONArray citations = spec.optJSONArray("citations");
        StringBuilder labels = new StringBuilder();
        if (citations != null) for (int i = 0; i < citations.length(); i++) {
            if (labels.length() > 0) labels.append(" · ");
            labels.append(citations.getJSONObject(i).getString("label"));
        }
        footer = new TextView(context);
        footer.setText((code ? spec.optString("language", format) + " · " : "")
            + "D-pad: scroll" + (labels.length() == 0 ? "" : "\n" + labels));
        footer.setTextColor(muted);
        footer.setTextSize(11);
        addView(footer, new LayoutParams(-1, -2));
    }

    /** Update values without losing focus, scroll position or sibling GPU views. */
    void update(JSONObject spec, JSONObject palette) throws Exception {
        String title = spec.getString("title"), text = spec.getString("body");
        if (!heading.getText().toString().equals(title)) heading.setText(title);
        if (!body.getText().toString().equals(text)) body.setText(text);
        String format = spec.getString("format");
        boolean code = format.equals("code") || format.equals("diff");
        JSONArray citations = spec.optJSONArray("citations");
        StringBuilder labels = new StringBuilder();
        if (citations != null) for (int i = 0; i < citations.length(); i++) {
            if (labels.length() > 0) labels.append(" · ");
            labels.append(citations.getJSONObject(i).getString("label"));
        }
        String caption = (code ? spec.optString("language", format) + " · " : "")
            + "D-pad: scroll" + (labels.length() == 0 ? "" : "\n" + labels);
        if (!footer.getText().toString().equals(caption)) footer.setText(caption);
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }
}
