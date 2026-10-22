local candle = event.payload

return {
  kind = "quant",
  mutation = {
    operation = "append_candle",
    candle = {
      time = candle.ts,
      open = candle.o,
      high = candle.h,
      low = candle.l,
      close = candle.c,
      volume = candle.v,
    },
  },
}
