use owo_colors::OwoColorize;

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
    println!("   {}", "LEGENDARY - APT NEXT GEN".white().bold());
    println!("{}", "   ────────────────────────".dimmed());
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
    println!("  {:<15} {}", "install".green(), "Install packages");
    println!("  {:<15} {}", "remove".red(), "Remove packages");
    println!("  {:<15} {}", "upgrade".blue(), "Upgrade system");
    println!("  {:<15} {}", "full-upgrade".blue().bold(), "Smart system upgrade");
    println!("  {:<15} {}", "autoremove".red().dimmed(), "Remove unused deps");
    println!();
    println!("{}", "QUERY COMMANDS:".yellow().bold());
    println!("  {:<15} {}", "search".magenta(), "Search packages (supports Regex)");
    println!("  {:<15} {}", "show".cyan(), "Show detailed info");
    println!("  {:<15} {}", "list".white(), "List all packages");
    println!();
    println!("{}", "REPO MANAGEMENT:".yellow().bold());
    println!("  {:<15} {}", "list-repos".white(), "Show active repositories");
    println!("  {:<15} {}", "add-repo".green(), "Add repository line");
    println!();
}
