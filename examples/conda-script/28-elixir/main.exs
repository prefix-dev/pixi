# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "elixir ${SCRIPT}"
#
# [dependencies]
# elixir = "*"
# /// end-conda-script

Mix.install([{:jason, "~> 1.4"}])

json = ~s({"tool":"conda-script","answer":42})
map = Jason.decode!(json)
IO.puts("#{map["tool"]} says #{map["answer"]}")
IO.puts(Jason.encode!([1, [2, 3], %{"ok" => true}]))
