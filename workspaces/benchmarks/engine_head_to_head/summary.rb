# Medians, p95 where twenty samples support it, and dispersion for every timed series of a run.
require_relative 'support'

module HeadToHead
  module Summary
    P95_MINIMUM = 20

    def self.statistics(values)
      return nil if values.empty?

      { n: values.size, median: HeadToHead.median(values), min: values.min, max: values.max,
        p95: values.size >= P95_MINIMUM ? HeadToHead.percentile(values, 0.95) : nil,
        q1: HeadToHead.percentile(values, 0.25), q3: HeadToHead.percentile(values, 0.75), samples: values }
    end

    # One series per arm, stage, state, role, engine and census jobs setting, so a fresh recomputation never joins
    # its warm edit; LSP sessions add one series per protocol mark.
    def self.series(samples)
      grouped = Hash.new { |hash, key| hash[key] = [] }
      samples.each do |sample|
        next unless sample['timed'] && %w[ordinary native-control].include?(sample['role'])

        key = sample.values_at('arm', 'stage', 'state', 'role', 'engine', 'jobs')
        grouped[key] << sample['wall_ms'] if sample['wall_ms']
        (sample['marks_ms'] || {}).each { |mark, value| grouped[key + [mark]] << value }
      end
      grouped.map do |key, values|
        arm, stage, state, role, engine, jobs, mark = key
        { arm: arm, stage: stage, state: state, role: role, engine: engine, jobs: jobs, mark: mark }.compact
          .merge(milliseconds: statistics(values))
      end
    end

    def self.write(output)
      samples = File.readlines(File.join(output, 'samples.jsonl')).map { |line| JSON.parse(line) }
      failures = File.readlines(File.join(output, 'failures.jsonl')).map { |line| JSON.parse(line) }
      parity = samples.select { |sample| %w[reply-parity snapshot-stability fresh-parity view-isolation].include?(sample['role']) }
      census = samples.select { |sample| sample['arm'] == 'census' && sample['role'] == 'ordinary' }
      summary = { samples: samples.size, failures: failures.map { |failure| failure.slice('arm', 'refusal') },
                  pair_orders: samples.select { |s| s['pair'] && s['role'] == 'ordinary' }.map { |s| s.values_at('arm', 'pair', 'position', 'engine') }.uniq,
                  parity_failures: parity.count { |sample| sample['equal'] == false || sample['isolated'] == false },
                  census: census.map { |run| run.slice('pair', 'position', 'engine', 'jobs', 'stage', 'exit', 'verdict_counts', 'cases', 'verdicts_sha256', 'oracle_equal', 'missing_passes', 'wall_ms') },
                  series: series(samples) }
      HeadToHead.record(File.join(output, 'summary.json'), summary)
      File.write(File.join(output, 'summary.md'), table(summary))
      summary
    end

    def self.table(summary)
      rows = summary.fetch(:series).map do |row|
        stats = row.fetch(:milliseconds)
        p95 = stats.fetch(:p95) ? format('%.1f', stats.fetch(:p95)) : 'n<20'
        "| #{row.values_at(:arm, :stage, :state, :role, :engine, :jobs, :mark).map { |value| value || '' }.join(' | ')} | " \
          "#{stats.fetch(:n)} | #{format('%.1f', stats.fetch(:median))} | #{p95} | #{format('%.1f', stats.fetch(:min))} | #{format('%.1f', stats.fetch(:max))} |"
      end
      header = "| Arm | Stage | State | Role | Engine | Jobs | Mark | n | Median ms | p95 ms | Min ms | Max ms |\n" \
               "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n"
      failures = summary.fetch(:failures).map { |failure| "- #{failure.fetch('arm')}: #{failure.fetch('refusal')}" }
      "# Engine head-to-head summary\n\n#{header}#{rows.join("\n")}\n\n" \
        "Failures: #{failures.empty? ? 'none' : "\n#{failures.join("\n")}"}\n"
    end
  end
end
