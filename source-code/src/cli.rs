use owo_colors::OwoColorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use std::thread;

pub const LOGO: &str = r#"
               *#                           @@@@@@@@@@                              ++              
               #**          @@       @@@@@@@         @@@@@@@@@       @@           ++**              
          ##   #***              @@@                          @@ @               *****              
          ***  ###**#          @@@      ###  ##                  @@@      @     ****##  *+          
          ##**#%%###***     @@@      @   ##########***      @@      @@@   @   *****#%% ***          
          %%#**#%%###**#  @@@             @%##%%%%####*#               @     **##*#%%#**##          
          %%##*##%%######*%%          @@   %%%%%%%%%%%####      %%%@       *#####%%%##*##%          
           %%#####%%%%####***#   @@      @%%%%%%%%% *#%%%%##%    @%   ####%%###%%@%####%%           
          ##%%%%%###%%%%%%###***          @%%%%%%##%%%%%%%%###       ##%%%%%%%@@%%###%%@            
          %%%%%%%%%%%%%@%%@%%##**#       %%%%%%%%%%###########*    ###%%%%@@%@%%##%%%%%%%%          
          @%%%%%@%%%%%%%@%%@%%##**##     %%%%%%%%@@@@@  @   @##  ####%%%%@@%@%%%%%%@@%%%@@          
           @@%%%%@@@@%%%@%%%%@%%####     @@%%%%%%#%%@            #%%%%@@@@@@@@@@@@@@%@@@            
             @@@@@@@@@   @@%%%@ %%##      @@%%%%%#####%         %%%%% @%%@@   @@@%@@@@              
           @@@@@@@@@@@@@@ @@@@%@%%%%%     @@%%%%%%%#**###  @@   %%%%%@@@@@@@@@@%@@@@ @@@@           
            @@@@@@@@@@@@@@@@@@@ @%%%%%      %%%%%%####**###     #%%%@@@@@@@@  @@@@@@@@@             
              @@@@@@@@@@@@@@@@@@ @%%%%%%    @@%%%%%####*####  ##%%%@ @@@@@@@@@@@@@@@@               
                 @@@@@@@@ @@@@@@@ @@@@%%@@@@@@@%%%%%%########%%%%%@@@@@@@@@@ @@@@@@                 
            @@   @@@@@@@@@@@@@@@@@ @@@@@@@@@@@@@%%%%%%#%%%#%%@@@@@@@@@@@@@@@@@@@@@@   @@            
               @@ @@@@@@@@@@@@@@@@@@@@@ @@@@@@@@@%%%%%%#%%%%%@@@@@@@@@@@@@@@@@@@@@ @                
               @@   @@@@@@ @@@@@@@@@ @@@@@@@@@@@@@@%%%%%%%%%%@@@@@@@@@@@@@@@@@    @@                
           @@  @@      @@@@@@@@   @@@@@@@@@@@@@@@@@@@%%%%%%@@@@@@@ @@@@@@@@@@@    @@                
                @@  @@  @@@@@  @@@    @@@@@@@@@@@@@@@@@@%%%@@@@@@@@@@@@@ @@@     @@@                
                @@  @@   @@@ @@@@@ @@@@@@@@@@@@@@@@@@@@@@%@@@@@@@@@@@@@@@    @@  @@@   @            
            @@               @@@ @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@            @   @              
                 @@      @@       @@   @@@@@@@@@@@@@@@@@@@@      @@      @@     @@@                 
                                     @@@@@@   @@@@@@@@@@@@                                          
"#;

pub fn print_header() {
    println!("{}", LOGO.cyan().dimmed());
    println!("   {}", "LEGENDARY ENGINE - APT NEXT GEN".white().bold());
    println!("{}", "   ───────────────────────────────────".dimmed());
}

pub fn log_step(phase: &str, msg: String) {
    println!(" {} {}: {}", "➜".blue().bold(), phase.bold(), msg);
}

pub fn log_success(msg: String) {
    println!(" {} {}", "✔".green().bold(), msg);
}

pub fn log_error(msg: String) {
    println!(" {} {}", "✘".red().bold(), msg);
}

pub fn log_warn(msg: String) {
    println!(" {} {}", "!".yellow().bold(), msg);
}

pub fn print_help() {
    print_header();
    println!("{}", "USAGE:".yellow().bold());
    println!("  legend <command> [arguments]");
    println!();
    println!("{}", "CORE COMMANDS:".yellow().bold());
    println!("  {:<15} {}", "install".green(), "Install packages (requires root)");
    println!("  {:<15} {}", "remove".red(), "Remove packages (requires root)");
    println!("  {:<15} {}", "update".blue(), "Refresh package index");
    println!("  {:<15} {}", "show".cyan(), "Show detailed package info");
    println!("  {:<15} {}", "search".magenta(), "Search for packages");
    println!("  {:<15} {}", "list".white(), "List all packages");
    println!("  {:<15} {}", "clean".yellow(), "Clean local cache");
    println!();
}

// Visual simulation of transaction until full Async callback bridge is ready
pub fn simulate_transaction_progress(action_name: &str, total_bytes: i64) {
    println!("{}", "─".repeat(60).dimmed());
    
    // Download Phase
    if total_bytes > 0 {
        log_step("Acquire", "Fetching archives...".to_string());
        let pb = ProgressBar::new(total_bytes as u64);
        pb.set_style(ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta}) {msg}")
            .unwrap()
            .progress_chars("━╸ "));
        
        let chunk_size = total_bytes / 50;
        for _ in 0..50 {
            pb.inc(chunk_size as u64);
            thread::sleep(Duration::from_millis(15)); // Simulating network lag
        }
        pb.finish_with_message("Done");
        log_success("Download complete".to_string());
    }

    // Install/Remove Phase
    log_step("Dpkg", format!("Running {} triggers...", action_name));
    
    let steps = vec![
        "Unpacking archive",
        "Verifying signature",
        "Preparing to configure",
        "Setting up configuration",
        "Processing triggers",
        "Finalizing"
    ];

    let pb = ProgressBar::new(steps.len() as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.magenta} {msg}")
        .unwrap()
        .tick_chars("⣾⣽⣻⢿⡿⣟⣯⣷ "));

    for step in steps {
        pb.set_message(format!("{}...", step));
        pb.inc(1);
        thread::sleep(Duration::from_millis(200));
    }
    pb.finish_and_clear();
}
