#!/usr/bin/ruby
# Reduce existing structured diagnostics; nested rustc phases are never summed with their parents.
require 'json'
require 'zlib'
require 'csv'

def invocation(path)
  record = JSON.parse(File.read(path))
  return nil unless record.key?('status')
  prefix = path.delete_suffix('.json')
  timers = Hash.new(0.0)
  boundaries = {}
  footprint = nil
  File.foreach(prefix+'.stderr') do |line|
    stamp, text = line.split("\t", 2)
    next unless text
    if text.start_with?('time: ')
      item = JSON.parse(text.delete_prefix('time: '))
      timers[item['pass']] += item['time']
    elsif text.start_with?('native-time: ')
      item = JSON.parse(text.delete_prefix('native-time: '))
      boundaries[item['boundary']] = item.merge('observed'=>stamp.to_f)
    elsif text.start_with?('footprint: ')
      footprint = JSON.parse(text.delete_prefix('footprint: '))
    end
  end
  spans = {}
  parsed = Hash.new(0)
  bodies = {}
  Dir.glob(prefix+'.traces/*.gz').each do |trace|
    Zlib::GzipReader.open(trace) do |input|
      input.each_line do |line|
        item = JSON.parse(line)
        if item['kind'] == 'begin'
          spans[item['id']] = item
          parsed[item.dig('fields', 'input')] += 1 if item['function'] == 'incan_driver::parse_desugar_source'
        elsif item['kind'] == 'end' && spans[item['id']]
          spans[item['id']]['end_ns'] = item['time_ns']
        elsif item.dig('fields', 'message') == 'checked direct-route Body IR'
          text = item.dig('fields', 'modules')
          bodies = {'bodies'=>text.scan(/\bBody \{/).size, 'statements'=>text.scan(/\bStatement \{/).size,
                    'locals'=>text.scan(/\bLocalDecl \{/).size}
        end
      end
    end
  end
  named = ->(name) { spans.values.find { |span| span['function'] == name && span['end_ns'] } }
  duration = ->(span) { span && (span['end_ns']-span['time_ns'])/1e9 }
  gap = ->(first, last) { first && last && (last['time_ns']-first['end_ns'])/1e9 }
  analysis = named.call('incan_rustc_driver::analyze_modules')
  graph = named.call('incan_driver::checked_unit_codegraph')
  lower = named.call('incan_rustc_driver::lower_program')
  root = named.call('incan_rustc_driver::compile_unit')
  unit = File.exist?(prefix+'.unit.json') ? JSON.parse(File.read(prefix+'.unit.json')) : {}
  frontend = analysis && root && (analysis['end_ns']-root['time_ns'])/1e9
  native = boundaries.dig('rustc_analysis', 'elapsed')
  llvm = if timers.key?('LLVM_passes') || timers.key?('LLVM_thinlto')
    timers['LLVM_passes']+timers['LLVM_thinlto']
  end
  record.merge('package'=>unit['package'], 'profile'=>unit['profile'], 'parsed_inputs'=>parsed,
               'source_lines'=>parsed.keys.compact.select { |file| File.file?(file) }.sum { |file| File.foreach(file).count },
               'checking'=>frontend, 'body_ir_context'=>gap.call(analysis, graph || lower),
               'codegraph'=>duration.call(graph), 'lowering'=>duration.call(lower),
               'rustc_analysis'=>native, 'llvm'=>llvm,
               'link'=>timers.key?('link') ? timers['link'] : nil,
               'footprint'=>footprint, 'body_counts'=>bodies, 'timers'=>timers)
end

base = ARGV.fetch(0)
paths = Dir.glob(base+'/commands/*.json').reject { |path| path.end_with?('.unit.json') }
incomplete = paths.reject { |path| JSON.parse(File.read(path)).key?('status') }
File.write(base+'/incomplete-invocations.json', JSON.pretty_generate(incomplete))
records = paths.map { |path| invocation(path) }.compact
File.write(base+'/invocations.json', JSON.pretty_generate(records))
columns = %w[trial package profile wall status checking body_ir_context codegraph lowering rustc_analysis llvm link footprint_bytes]
CSV.open(base+'/phases.tsv', 'w', col_sep: "\t") do |csv|
  csv << columns
  records.each do |record|
    csv << columns.map do |key|
      value = key == 'footprint_bytes' ? record.dig('footprint', 'bytes') : record[key]
      value.is_a?(Float) ? value.round(6) : value
    end
  end
end
File.open(base+'/summary.md', 'w') do |out|
  out.puts "| Trial | Command seconds | Status |\n| --- | ---: | ---: |"
  CSV.foreach(base+'/times.tsv', headers: true, col_sep: "\t") do |row|
    out.puts "| #{row['trial']} | #{format('%.3f', row['wall_seconds'])} | #{row['status']} |"
  end
end
puts "Retained #{records.length} invocations; see summary.md, phases.tsv and invocations.json"
