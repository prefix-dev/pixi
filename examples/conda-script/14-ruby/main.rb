# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "ruby ${SCRIPT}"
#
# [dependencies]
# ruby = "*"
# rb-addressable = "*"
# /// end-conda-script
require "addressable/template"

template = Addressable::Template.new("https://prefix.dev/{channel}/{package}{?platform}")
uri = template.expand("channel" => "conda-forge", "package" => "ruby", "platform" => "linux-64")
puts uri
parsed = Addressable::URI.parse(uri)
puts parsed.host
puts parsed.query_values["platform"]
