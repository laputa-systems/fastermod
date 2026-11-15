module A where

helper :: Int -> Int
helper x = x + 1

main :: IO ()
main = do
  let count = helper 1
  let helper = count
  print helper
  print count
