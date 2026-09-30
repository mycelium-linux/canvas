local book = event.payload
if type(book) ~= "table" or type(book.bids) ~= "table" or type(book.asks) ~= "table" then
  error("market-order-book-v1 requires bids and asks")
end
local function levels(source)
  local result = canvas.array()
  for _, level in ipairs(source) do
    result[#result + 1] = {
      price = tonumber(level.price or level[1]),
      size = tonumber(level.size or level[2]),
      orders = tonumber(level.orders or level[3]) or 0,
    }
  end
  return result
end
return {kind="quant",mutation={operation="replace_book",bids=levels(book.bids),asks=levels(book.asks)}}
