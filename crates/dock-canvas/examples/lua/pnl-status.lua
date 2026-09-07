local sign = module.inputs.pnl >= 0 and "+" or ""

canvas.text {
  id = "pnl",
  text = string.format("P/L  %s%.2f", sign, module.inputs.pnl)
}

canvas.text {
  id = "status",
  text = string.format(
    "%s · %s/%s",
    module.inputs.status.label,
    module.theme.mode,
    module.theme.accent
  ),
  translation = { 0, -0.3, 0 }
}
