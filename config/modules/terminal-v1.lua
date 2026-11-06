-- Terminal Spore: how this window behaves, not how the compositor behaves.
--
-- Canvas owns the pixels and the pty; the Spore owns taste. A bell is a sound, a
-- sound is a file, and whether a cursor blinks is a preference -- none of which
-- belongs in a compositor's source.
return {
  cursor = {
    blink = true,
    -- Milliseconds per phase. 530 is the classic VT blink; slower reads as lag,
    -- faster reads as a busy indicator rather than a cursor.
    period_ms = 530,
  },
  bell = {
    enabled = true,
    sound = "/System/Library/Sounds/Glass.aiff",
    volume = 0.35,
  },
}
