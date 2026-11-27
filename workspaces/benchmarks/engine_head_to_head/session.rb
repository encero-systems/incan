# One comparison run: its settings, frozen inputs, output tree and sample log.
require_relative 'support'

module HeadToHead
  class Session
    attr_reader :settings, :output, :timed, :slot_root, :home, :launcher, :lsp, :prefill, :source

    def initialize(settings)
      @settings = settings
      @output = settings.fetch(:output)
      @timed = settings.fetch(:timed)
      @slot_root = settings[:slot_root]
      @home = settings.fetch(:home)
      @launcher = File.join(@home, 'bin/incan')
      @lsp = File.join(@home, 'bin/incan-lsp')
      @prefill = settings.fetch(:prefill)
      @source = settings.fetch(:source)
      @spill = HeadToHead.make_directory(File.join(@output, 'streams'))
      @samples = File.open(File.join(@output, 'samples.jsonl'), 'a', 0o600)
      @samples.sync = true
      @failures = File.open(File.join(@output, 'failures.jsonl'), 'a', 0o600)
      @failures.sync = true
    end

    # Pair N runs Legacy first when N is odd and Records first when N is even.
    def order(pair)
      pair.odd? ? ENGINES : ENGINES.reverse
    end

    def directory(*parts)
      HeadToHead.make_directory(File.join(@output, *parts))
    end

    # The ordinary environment differs between arms only by the engine selector and the arm's own Bakery.
    def environment(engine, bakery)
      { 'INCAN_ENGINE' => engine, 'INCAN_BAKERY' => bakery }
    end

    # Observe one child and append it to the samples with its scope. Returns the logged entry.
    def sample(scope, directory, environment, argv, timed: @timed, input: nil)
      observation = HeadToHead.observe(directory, environment, argv, timed: timed, slot_root: @slot_root, input: input)
      entry = scope.merge(timed: timed).merge(observation)
      entry[:stdout] = HeadToHead.stream_entry(observation.fetch(:stdout), @spill, 'stdout')
      entry[:stderr] = HeadToHead.stream_entry(observation.fetch(:stderr), @spill, 'stderr')
      entry[:raw_stdout] = observation.fetch(:stdout)
      log(entry)
      entry
    end

    def log(entry)
      @samples.puts(JSON.generate(entry.reject { |key, _| key == :raw_stdout }))
    end

    # The first failing ordinary command of an arm, kept with its evidence; the arm stops there.
    def fail_arm(arm, refusal)
      evidence = refusal.evidence
      evidence = evidence.reject { |key, _| key == :raw_stdout } if evidence.is_a?(Hash)
      @failures.puts(JSON.generate({ arm: arm, utc: HeadToHead.utc, refusal: refusal.message, evidence: evidence }))
    end

    # With --retire-trees, a pair that passed gives back its Bakeries, targets and scratch; logs and samples stay.
    def retire(root)
      return unless @settings[:retire_trees]

      bulky = Dir.glob(File.join(root, '**', '{bakery,target,*-scratch,*-tmp}')).select { |path| File.directory?(path) }
      bulky = bulky.reject { |path| bulky.any? { |other| other != path && path.start_with?(other + '/') } }
      kib = bulky.sum { |path| Open3.capture2('/usr/bin/du', '-sk', path).first.to_i }
      bulky.each { |path| FileUtils.rm_rf(path) }
      log({ role: 'retired-trees', root: root, paths: bulky.size, kib: kib })
    end

    def succeeded?(entry, expected_stdout: nil)
      entry.fetch(:exit) == 0 && entry.fetch(:signal).nil? &&
        (expected_stdout.nil? || entry.fetch(:raw_stdout) == expected_stdout)
    end

    def close
      @samples.close
      @failures.close
    end
  end
end
