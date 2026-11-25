require "json"
require "yaml"

known_labels = JSON.parse($stdin.read).map { |label| label.fetch("name") }
abort "The repository label inventory is empty" if known_labels.empty?

errors = Dir.glob(".github/ISSUE_TEMPLATE/*.md").flat_map do |path|
  frontmatter = File.read(path).match(/\A---\s*\n(.*?)^---\s*$/m)
  abort "#{path}: missing YAML frontmatter" unless frontmatter

  defaults = YAML.safe_load(frontmatter[1]).fetch("labels", [])
  defaults = defaults.split(",") if defaults.is_a?(String)
  Array(defaults).map(&:strip).reject(&:empty?).map do |label|
    "#{path}: unknown label #{label.inspect}" unless known_labels.include?(label)
  end.compact
end

abort errors.join("\n") unless errors.empty?
puts "Issue template labels match the repository inventory"
