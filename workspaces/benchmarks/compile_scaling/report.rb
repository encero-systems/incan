#!/usr/bin/ruby
# Reduce existing structured diagnostics; nested rustc phases are never summed with their parents.
require 'json'
require 'zlib'
require 'csv'
require 'set'

# These are execution observations, distinct from inventories of retained products.
def record_work(work, phase, input, kind, source = nil)
  entry = work[phase] ||= {executions: 0, inputs: Set.new, by_kind: Hash.new(0), by_source: Hash.new(0)}
  entry[:executions] += 1
  entry[:inputs] << input
  entry[:by_kind][kind] += 1
  entry[:by_source][source] += 1 if source
end

def reduced_work(work)
  work.transform_values do |entry|
    {'executions'=>entry[:executions], 'unique_inputs'=>entry[:inputs].size,
     'by_kind'=>entry[:by_kind], 'by_source'=>entry[:by_source], 'cache_hits'=>nil}
  end
end

def invocation(path)
  record = JSON.parse(File.read(path))
  return nil unless record.key?('status')
  return nil if record['role'] == 'daemon' || record.dig('argv', 0) == '--resident'
  prefix = path.delete_suffix('.json')
  timers = Hash.new(0.0)
  boundaries = {}
  footprint = nil
  work = {}
  work_observed = false
  lowering_observed = false
  codegen = nil
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
    elsif text.start_with?('redo-item: lowering' + "\t")
      fields = text.rstrip.split("\t", 5)
      record_work(work, 'functions_lowered', fields.drop(1), 'concrete_frame', fields[4])
    elsif text.start_with?('redo-codegen: ')
      codegen = JSON.parse(text.delete_prefix('redo-codegen: '))
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
        elsif item.dig('fields', 'message') == 'compiler work observations'
          work_observed = item.dig('fields', 'schema') == 1
          lowering_observed = item.dig('fields', 'lowering_observed') == true
        elsif item.dig('fields', 'message') == 'checked direct-route Body IR'
          text = item.dig('fields', 'modules')
          bodies = {'bodies'=>text.scan(/\bBody \{/).size, 'statements'=>text.scan(/\bStatement \{/).size,
                    'locals'=>text.scan(/\bLocalDecl \{/).size}
        elsif item.dig('fields', 'message') == 'checked Body IR inventory'
          bodies = item['fields'].slice('modules', 'module_body_entries')
        elsif item.dig('fields', 'message') == 'compiler work'
          fields = item['fields']
          phase = {['checking', 'header_visit']=>'header_visits',
                   ['checking', 'declaration']=>'declarations_checked', ['checking', 'body']=>'checked_bodies',
                   ['body_ir', 'built']=>'body_ir_built', ['body_ir', 'nested']=>'nested_body_ir_built'}[
                     [fields['phase'], fields['action']]]
          input = fields.values_at('module', 'input', 'identity', 'kind')
          record_work(work, phase, input, fields['kind']) if phase
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
  codegen_sent = if record['status'] == 0 && codegen
    case codegen['execution']
    when 'all_on_success' then codegen['partitions']
    when 'none' then 0
    end
  end
  llvm = if timers.key?('LLVM_passes') || timers.key?('LLVM_thinlto')
    timers['LLVM_passes']+timers['LLVM_thinlto']
  end
  if work_observed
    phases = %w[header_visits declarations_checked checked_bodies body_ir_built nested_body_ir_built]
    phases << 'functions_lowered' if lowering_observed
    phases.each do |phase|
      work[phase] ||= {executions: 0, inputs: Set.new, by_kind: {}, by_source: {}}
    end
  end
  record.merge('package'=>unit['package'], 'profile'=>unit['profile'], 'parsed_inputs'=>parsed,
               'source_lines'=>parsed.keys.compact.select { |file| File.file?(file) }.sum { |file| File.foreach(file).count },
               'checking'=>frontend, 'body_ir_context'=>gap.call(analysis, graph || lower),
               'codegraph'=>duration.call(graph), 'lowering'=>duration.call(lower),
               'rustc_analysis'=>native, 'llvm'=>llvm,
               'link'=>timers.key?('link') ? timers['link'] : nil,
               'footprint'=>footprint, 'body_counts'=>bodies, 'work_observations'=>work_observed,
               'work_counts'=>reduced_work(work),
               'codegen_counts'=>codegen && codegen.merge('sent_to_llvm'=>codegen_sent),
               'timers'=>timers)
end

base = ARGV.fetch(0)
paths = Dir.glob(base+'/commands/*.json').reject { |path| path.end_with?('.unit.json') }
incomplete = paths.reject { |path| JSON.parse(File.read(path)).key?('status') }
File.write(base+'/incomplete-invocations.json', JSON.pretty_generate(incomplete))
daemons = paths.map { |path| JSON.parse(File.read(path)) }.select { |record| record.dig('argv', 0) == '--resident' }
File.write(base+'/daemon-invocations.json', JSON.pretty_generate(daemons))
records = paths.map { |path| invocation(path) }.compact
File.write(base+'/invocations.json', JSON.pretty_generate(records))
totals = CSV.read(base+'/times.tsv', headers: true, col_sep: "\t").map do |trial|
  selected = records.select { |record| record['trial'] == trial['trial'] }
  semantic_modes = %w[--unit --check --codegraph --source-interface]
  incan = selected.select { |record| semantic_modes.include?(record.dig('argv', 0)) }
  known_modes = semantic_modes + %w[--rust-compile --rust-compile-codegraph --rust-abi --rust-codegraph --read-codegraph --fmt --source-files --rust-imports --identity]
  unfinished = incomplete.any? do |path|
    record = JSON.parse(File.read(path))
    record['trial'] == trial['trial'] && record.dig('argv', 0) != '--resident'
  end
  known = daemons.empty? && !unfinished && selected.all? { |record| known_modes.include?(record.dig('argv', 0)) }
  phases = %w[header_visits declarations_checked checked_bodies body_ir_built nested_body_ir_built functions_lowered]
  counts = phases.map do |phase|
    available = known && incan.all? { |record| record['work_observations'] && record['work_counts'].key?(phase) }
    [phase, available ? incan.sum { |record| record.dig('work_counts', phase, 'executions').to_i } : nil]
  end.to_h
  native = selected.select { |record| %w[--unit --rust-compile --rust-compile-codegraph].include?(record.dig('argv', 0)) }
  counts['llvm_codegen_units'] = if known && native.all? { |record| record.dig('codegen_counts', 'sent_to_llvm') }
    native.sum { |record| record.dig('codegen_counts', 'sent_to_llvm') }
  end
  {'trial'=>trial['trial'], 'compiler_invocations'=>selected.size,
   'resident_work_unavailable'=>!daemons.empty?, 'unfinished_invocation'=>unfinished,
   'by_mode'=>selected.group_by { |record| record.dig('argv', 0) }.transform_values(&:size), 'executions'=>counts}
end
File.write(base+'/work-totals.json', JSON.pretty_generate(totals))
columns = %w[trial package profile wall postprocess_wall collector_wall status checking body_ir_context codegraph lowering rustc_analysis llvm link footprint_bytes header_visits declarations_checked checked_bodies body_ir_built nested_body_ir_built functions_lowered llvm_codegen_units]
CSV.open(base+'/phases.tsv', 'w', col_sep: "\t") do |csv|
  csv << columns
  records.each do |record|
    csv << columns.map do |key|
      value = if key == 'footprint_bytes'
        record.dig('footprint', 'bytes')
      elsif key == 'llvm_codegen_units'
        record.dig('codegen_counts', 'sent_to_llvm')
      elsif record['work_counts'].key?(key)
        record.dig('work_counts', key, 'executions')
      else
        record[key]
      end
      value.is_a?(Float) ? value.round(6) : value
    end
  end
end
File.open(base+'/summary.md', 'w') do |out|
  out.puts "| Trial | Command seconds | Status |\n| --- | ---: | ---: |"
  CSV.foreach(base+'/times.tsv', headers: true, col_sep: "\t") do |row|
    seconds = row['wall_seconds']
    seconds = seconds.to_s.empty? ? 'unavailable' : format('%.3f', seconds)
    out.puts "| #{row['trial']} | #{seconds} | #{row['status']} |"
  end
end
puts "Retained #{records.length} invocations; see summary.md, phases.tsv and invocations.json"
